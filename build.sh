#!/bin/sh

# Build each demo into build/<demo>.elf.

set -e

SYMBOLASM=${SYMBOLASM:-../symbolasm}
TILESET=${TILESET:-maps/alien_world.png}
PIOASM=${PIOASM:-$HOME/dl/pico-examples/build/pioasm/pioasm}

python3 tools/tiles.py "$TILESET" maps/colony.tmx > tiles.sy

mkdir -p build
# USB serial logging (USBLOG ring, usb_init, USBCTRL_IRQ), a Rust staticlib
(cd usblog && cargo build -q --release)
USBLOG=usblog/target/thumbv8m.main-none-eabihf/release/libusblog.a
# A demo with its app in Rust, which brings usblog along
(cd drag && cargo build -q --release)
DRAG=drag/target/thumbv8m.main-none-eabihf/release/libdrag.a
# The register definitions come with the rp235x-pac crate usblog depends on
if [ -z "$SVD" ]; then
    SVD=$(cd usblog && cargo metadata -q --format-version 1 | python3 -c '
import json, os, sys
pac = next(p for p in json.load(sys.stdin)["packages"] if p["name"] == "rp235x-pac")
print(os.path.join(os.path.dirname(pac["manifest_path"]), "svd", "RP2350.svd"))')
fi
cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- fill.sy > build/fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/fill.o build/fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/startup.o startup.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/image_data.o image.s

# demo name, its .sy files (compiled with main.sy), its other objects, and
# the Rust library, if it isn't usblog
demo() {
    cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- main.sy log.sy psram.sy usb_crc.sy usb_tx.sy usb_rx.sy usb_host.sy $2 --svd "$SVD" > "build/$1.s"
    arm-none-eabi-as -mcpu=cortex-m33 -o "build/$1.o" "build/$1.s"
    # --gc-sections drops the cortex-m-rt reset handler and vector table
    # that come with rp235x-hal
    arm-none-eabi-ld -T link.ld build/startup.o "build/$1.o" $3 "${4:-$USBLOG}" \
        --gc-sections --no-warn-rwx-segments -z noexecstack -o "build/$1.elf"
}

demo colorbars "vec.sy render.sy display_list.sy stats.sy colorbars.sy pico_alloc.sy alloc.sy" build/fill.o
demo rects "vec.sy render.sy display_list.sy stats.sy rects.sy pico_alloc.sy alloc.sy" build/fill.o
demo drag "vec.sy render.sy display_list.sy stats.sy pico_alloc.sy alloc.sy" build/fill.o "$DRAG"
demo textdemo "vec.sy render.sy display_list.sy stats.sy font.sy text.sy textdemo.sy pico_alloc.sy alloc.sy" build/fill.o
demo console "vec.sy render.sy display_list.sy stats.sy font.sy text.sy console.sy pico_alloc.sy alloc.sy" build/fill.o
demo image image.sy build/image_data.o
demo tiles "render_tiles.sy tiles.sy"
demo psram_test psram_test.sy
"$PIOASM" -o json sd.pio | python3 tools/pio2sy.py sd_pio_program > sd_pio.sy
demo sd_test "sd_pio.sy sd.sy sd_test.sy fat.sy"
demo sd_image "sd_pio.sy sd.sy fat.sy sd_image.sy"
demo sd_video "sd_pio.sy sd.sy fat.sy sd_video.sy"
demo sd_bench "sd_pio.sy sd.sy fat.sy sd_bench.sy"
