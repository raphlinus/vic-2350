@ Copyright 2026 Raph Levien
@ SPDX-License-Identifier: Apache-2.0 OR MIT

@ Sections that don't have symbolasm syntax yet: the RP2350 image
@ definition block and the vector table.

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
