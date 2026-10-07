@ Copyright 2026 Raph Levien
@ SPDX-License-Identifier: Apache-2.0 OR MIT

@ Sections that don't have symbolasm syntax yet: the RP2350 image
@ definition block, the vector table, and placeholder .data/.bss (the
@ linker script asserts both are nonempty).

.cpu cortex-m33
.syntax unified
.thumb

.equ STACK_TOP_ADDRESS, 0x20080000

@ --- RP2350 Image Definition Block (IDB) ---
.section .image_def, "ax"
.align 2
.word 0xffffded3
.word 0x10210142
.word 0x000001ff
.word 0x00000000
.word 0xab123579

.section .vectors, "ax"
.align 2
.word STACK_TOP_ADDRESS           @ Initial Stack Pointer (Top of RAM)
.word start                       @ Reset Handler

.section .data
    .space 4

.section .bss
    .space 4
