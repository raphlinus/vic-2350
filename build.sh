#!/bin/sh

set -e

SYMBOLASM=${SYMBOLASM:-$HOME/garden/symbolasm}
SVD=${SVD:-$HOME/dl/pico-sdk/src/rp2350/hardware_regs/RP2350.svd}

cargo run -q --manifest-path "$SYMBOLASM/Cargo.toml" -- main.sy --svd "$SVD" > main.s
arm-none-eabi-as -mcpu=cortex-m33 -o main.o main.s
arm-none-eabi-as -mcpu=cortex-m33 -o startup.o startup.s
arm-none-eabi-ld -T link.ld startup.o main.o --no-warn-rwx-segments -o main.elf
