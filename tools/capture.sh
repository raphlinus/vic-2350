#!/bin/sh
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Grab the DVI output from the HDMI capture device: save a frame as a PNG
# and report rows of red err_line pixels.
#
# Usage: tools/capture.sh OUT.png [FRAMES [WIDTHxHEIGHT:CODEC]]
#
# Exits 0 if no frame had red rows, 3 if some did, 1 on errors.
#
# The capture tool is built as build/Capture.app and launched with open,
# so macOS asks for camera access for it (once) rather than silently
# refusing a command line tool. An ad hoc signature changes with each
# build, which loses the grant, so a development identity is used if
# there is one.

set -e

dir=$(cd "$(dirname "$0")/.." && pwd)
app="$dir/build/Capture.app"
bin="$app/Contents/MacOS/capture"
if [ ! -x "$bin" ] || [ "$dir/tools/capture.swift" -nt "$bin" ]; then
    mkdir -p "$app/Contents/MacOS"
    swiftc -O -o "$bin" "$dir/tools/capture.swift"
    cat > "$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>org.levien.vic.capture</string>
    <key>CFBundleName</key>
    <string>Capture</string>
    <key>CFBundleExecutable</key>
    <string>capture</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSCameraUsageDescription</key>
    <string>Capture DVI output for vic-2350 tests</string>
</dict>
</plist>
EOF
    identity=$(security find-identity -v -p codesigning | sed -n 's/^ *1) \([0-9A-F]*\) .*/\1/p')
    codesign -s "${identity:--}" --force "$app" 2>/dev/null
fi

out=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
log="$out.log"
rm -f "$out.rc" "$log"
open -W -n --stdout "$log" --stderr "$log" "$app" --args "$out" ${2:+"$2"} ${3:+"$3"}
cat "$log" 2>/dev/null || true
rc=$(cat "$out.rc" 2>/dev/null || echo 1)
rm -f "$out.rc" "$log"
exit "$rc"
