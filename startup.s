@ Copyright 2026 Raph Levien
@ SPDX-License-Identifier: Apache-2.0 OR MIT

@ Sections that don't have symbolasm syntax yet: the RP2350 image
@ definition block, the vector table, and the initialized DMA command
@ lists in .data.

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
.word default_handler // nmi handler
.word default_handler // hardfault_handler
.word default_handler // memmanage_handler
.word default_handler // busfault_handler
.word default_handler // usagefault_handler
.word default_handler // securefault_handler
.word 0
.word 0
.word 0
.word default_handler // svc_handler
.word default_handler // debugmon_handler
.word 0
.word default_handler // pendsv_handler
.word default_handler // systick_handler

// interrupts
.word default_handler // interrupt 0, TIMER0_IRQ_0
.word default_handler // interrupt 1, TIMER0_IRQ_1
.word default_handler // interrupt 2, TIMER0_IRQ_2
.word default_handler // interrupt 3, TIMER0_IRQ_3
.word default_handler // interrupt 4, TIMER1_IRQ_0
.word default_handler // interrupt 5, TIMER1_IRQ_1
.word default_handler // interrupt 6, TIMER1_IRQ_2
.word default_handler // interrupt 7, TIMER1_IRQ_3
.word default_handler // interrupt 8, PWM_IRQ_WRAP_0
.word default_handler // interrupt 9, PWM_IRQ_WRAP_1
.word dma_irq_0_handler // interrupt 10, DMA_IRQ_0

@ --- HSTX command lists for 640x480 DVI, read by DMA ---
.equ TMDS_CTRL_00, 0x354
.equ TMDS_CTRL_01, 0x0ab
.equ TMDS_CTRL_10, 0x154
.equ TMDS_CTRL_11, 0x2ab

.equ SYNC_V0_H0, TMDS_CTRL_00 | (TMDS_CTRL_00 << 10) | (TMDS_CTRL_00 << 20)
.equ SYNC_V0_H1, TMDS_CTRL_01 | (TMDS_CTRL_00 << 10) | (TMDS_CTRL_00 << 20)
.equ SYNC_V1_H0, TMDS_CTRL_10 | (TMDS_CTRL_00 << 10) | (TMDS_CTRL_00 << 20)
.equ SYNC_V1_H1, TMDS_CTRL_11 | (TMDS_CTRL_00 << 10) | (TMDS_CTRL_00 << 20)

.section .data
    .balign 4
.global blank_line_vsync_off
blank_line_vsync_off:
    .word (1 << 12) | 16 // raw repeat, h_front_porch
    .word SYNC_V1_H1
    .word (1 << 12) | 96 // raw repeat, h_sync
    .word SYNC_V1_H0
    .word (1 << 12) | 688 // raw repeat, h_back_porch + active
    .word SYNC_V1_H1
    .word 0xf000 // nop; this probably isn't the best thing to do, but copying sources

.global blank_line_vsync_on
blank_line_vsync_on:
    .word (1 << 12) | 16 // raw repeat, h_front_porch
    .word SYNC_V0_H1
    .word (1 << 12) | 96 // raw repeat, h_sync
    .word SYNC_V0_H0
    .word (1 << 12) | 688 // raw repeat, h_back_porch + active
    .word SYNC_V0_H1
    .word 0xf000

.global line_active
line_active:
    .word (1 << 12) | 16 // raw repeat, h_front_porch
    .word SYNC_V1_H1
    .word 0xf000
    .word (1 << 12) | 96 // raw repeat, h_sync
    .word SYNC_V1_H0
    .word (1 << 12) | 48 // raw repeat, h_back_porch
    .word SYNC_V1_H1
    .word (3 << 12) | 640 // tmds repeat, active
    .word 0x03e003e0
