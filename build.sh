#!/bin/sh

# Build each demo into build/<demo>.elf.

set -e

SYMBOLASM=${SYMBOLASM:-../symbolasm}
TILESET=${TILESET:-maps/alien_world.png}

python3 tools/tiles.py "$TILESET" maps/colony.tmx > tiles.sy

mkdir -p build
# USB serial logging (USBLOG ring, usb_init, USBCTRL_IRQ), a Rust staticlib
(cd usblog && cargo build -q --release)
USBLOG=usblog/target/thumbv8m.main-none-eabihf/release/libusblog.a
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

# demo name, its .sy files (compiled with main.sy), and its other objects
demo() {
    cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- main.sy log.sy psram.sy $2 --svd "$SVD" > "build/$1.s"
    arm-none-eabi-as -mcpu=cortex-m33 -o "build/$1.o" "build/$1.s"
    # --gc-sections drops the cortex-m-rt reset handler and vector table
    # that come with rp235x-hal
    arm-none-eabi-ld -T link.ld build/startup.o "build/$1.o" $3 "$USBLOG" \
        --gc-sections --no-warn-rwx-segments -z noexecstack -o "build/$1.elf"
}

demo colorbars "vec.sy render.sy display_list.sy stats.sy colorbars.sy pico_alloc.sy alloc.sy" build/fill.o
demo rects "vec.sy render.sy display_list.sy stats.sy rects.sy pico_alloc.sy alloc.sy" build/fill.o
demo textdemo "vec.sy render.sy display_list.sy stats.sy font.sy text.sy textdemo.sy pico_alloc.sy alloc.sy" build/fill.o
demo console "vec.sy render.sy display_list.sy stats.sy font.sy text.sy console.sy pico_alloc.sy alloc.sy" build/fill.o
demo image image.sy build/image_data.o
demo tiles "render_tiles.sy tiles.sy"
demo psram_test psram_test.sy
