#!/bin/sh

# Build each demo into build/<demo>.elf.

set -e

SYMBOLASM=${SYMBOLASM:-$HOME/garden/symbolasm}
SVD=${SVD:-$HOME/dl/pico-sdk/src/rp2350/hardware_regs/RP2350.svd}
TILESET=${TILESET:-$HOME/Downloads/dandelion_tileset.png}

python3 tools/tiles.py "$TILESET" > tiles.sy

mkdir -p build
cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- fill.sy > build/fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/fill.o build/fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/startup.o startup.s
arm-none-eabi-as -mcpu=cortex-m33 -o build/image_data.o image.s

# demo name, its .sy files (compiled with main.sy), and its other objects
demo() {
    cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- main.sy $2 --svd "$SVD" > "build/$1.s"
    arm-none-eabi-as -mcpu=cortex-m33 -o "build/$1.o" "build/$1.s"
    arm-none-eabi-ld -T link.ld build/startup.o "build/$1.o" $3 --no-warn-rwx-segments -o "build/$1.elf"
}

demo colorbars "render.sy colorbars.sy pico_alloc.sy" build/fill.o
demo image image.sy build/image_data.o
demo tiles "render_tiles.sy tiles.sy"
