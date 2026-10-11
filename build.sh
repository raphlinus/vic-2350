#!/bin/sh

set -e

SYMBOLASM=${SYMBOLASM:-$HOME/garden/symbolasm}
SVD=${SVD:-$HOME/dl/pico-sdk/src/rp2350/hardware_regs/RP2350.svd}
TILESET=${TILESET:-$HOME/Downloads/dandelion_tileset.png}

python3 tools/tiles.py "$TILESET" > tiles.sy

cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- main.sy tiles.sy --svd "$SVD" > main.s
cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- fill.sy > fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o main.o main.s
arm-none-eabi-as -mcpu=cortex-m33 -o fill.o fill.s
arm-none-eabi-as -mcpu=cortex-m33 -o startup.o startup.s
arm-none-eabi-ld -T link.ld startup.o main.o fill.o --no-warn-rwx-segments -o main.elf
