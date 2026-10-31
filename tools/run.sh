#!/bin/sh
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Load an ELF onto the board, run it, and print its USB serial log until
# the "#END" marker (log_end in log.sy) or a timeout.
#
# Usage: tools/run.sh build/colorbars.elf [timeout_seconds]
#
# If a vic image is already running, a 1200 baud touch on its port puts
# it in BOOTSEL first, so no button press is needed. Exits 0 when #END
# was seen, 1 on timeout.

set -e

ELF=${1:?usage: tools/run.sh ELF [timeout_seconds]}
TIMEOUT=${2:-10}
PICOTOOL=${PICOTOOL:-picotool}
if ! command -v "$PICOTOOL" >/dev/null; then
    echo "run.sh: picotool not found; install it or set PICOTOOL" >&2
    exit 2
fi

# macOS names the port cu.usbmodem*, Linux ttyACM*
find_port() {
    ls /dev/cu.usbmodem* /dev/ttyACM* 2>/dev/null | head -n 1
}

in_bootsel() {
    "$PICOTOOL" info >/dev/null 2>&1
}

# wait_for COMMAND SECONDS
wait_for() {
    n=$(($2 * 10))
    while [ $n -gt 0 ]; do
        if $1; then return 0; fi
        sleep 0.1
        n=$((n - 1))
    done
    return 1
}

if ! in_bootsel; then
    port=$(find_port)
    if [ -z "$port" ]; then
        echo "run.sh: no board in BOOTSEL and no serial port; hold BOOTSEL and reset" >&2
        exit 2
    fi
    # Opening at 1200 baud and closing (dropping DTR) reboots into BOOTSEL.
    # BSD stty takes -f, GNU stty -F
    stty -f "$port" 1200 2>/dev/null || stty -F "$port" 1200 2>/dev/null || true
    if ! wait_for in_bootsel 10; then
        echo "run.sh: $port didn't reboot into BOOTSEL" >&2
        exit 2
    fi
fi

"$PICOTOOL" load -x -t elf "$ELF" >/dev/null

port_up() {
    [ -n "$(find_port)" ]
}
if ! wait_for port_up 10; then
    echo "run.sh: no serial port after loading $ELF" >&2
    exit 2
fi

exec python3 -I - "$(find_port)" "$TIMEOUT" <<'EOF'
import os, select, sys, termios, time

port, timeout = sys.argv[1], float(sys.argv[2])
fd = os.open(port, os.O_RDWR | os.O_NOCTTY)
attrs = termios.tcgetattr(fd)
attrs[0] = 0  # iflag
attrs[1] = 0  # oflag
attrs[3] = 0  # lflag: raw
termios.tcsetattr(fd, termios.TCSANOW, attrs)
end = b"\n#END\n"
seen = b""
deadline = time.monotonic() + timeout
while True:
    left = deadline - time.monotonic()
    if left <= 0:
        sys.stderr.write("run.sh: timed out\n")
        sys.exit(1)
    r, _, _ = select.select([fd], [], [], left)
    if not r:
        continue
    data = os.read(fd, 4096)
    if not data:
        continue
    seen += data
    if end in seen:
        sys.stdout.buffer.write(seen[: seen.index(end) + len(end)])
        sys.stdout.flush()
        sys.exit(0)
    # print complete lines as they arrive
    cut = seen.rfind(b"\n") + 1
    if cut and len(seen) > len(end):
        keep = min(cut, len(seen) - len(end))
        sys.stdout.buffer.write(seen[:keep])
        sys.stdout.flush()
        seen = seen[keep:]
EOF
