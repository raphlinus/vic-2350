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
.word default_handler // interrupt 11, DMA_IRQ_1
.word default_handler // interrupt 12, DMA_IRQ_2
.word default_handler // interrupt 13, DMA_IRQ_3
.word USBCTRL_IRQ // interrupt 14, USBCTRL_IRQ, in the usblog crate
.word default_handler // interrupt 15, PIO0_IRQ_0
.word default_handler // interrupt 16, PIO0_IRQ_1
.word default_handler // interrupt 17, PIO1_IRQ_0
.word default_handler // interrupt 18, PIO1_IRQ_1
.word default_handler // interrupt 19, PIO2_IRQ_0
.word default_handler // interrupt 20, PIO2_IRQ_1
.word default_handler // interrupt 21, IO_IRQ_BANK0
.word default_handler // interrupt 22, IO_IRQ_BANK0_NS
.word default_handler // interrupt 23, IO_IRQ_QSPI
.word default_handler // interrupt 24, IO_IRQ_QSPI_NS
.word default_handler // interrupt 25, SIO_IRQ_FIFO
.word default_handler // interrupt 26, SIO_IRQ_BELL
.word default_handler // interrupt 27, SIO_IRQ_FIFO_NS
.word default_handler // interrupt 28, SIO_IRQ_BELL_NS
.word default_handler // interrupt 29, SIO_IRQ_MTIMECMP
.word default_handler // interrupt 30, CLOCKS_IRQ
.word default_handler // interrupt 31, SPI0_IRQ
.word default_handler // interrupt 32, SPI1_IRQ
.word default_handler // interrupt 33, UART0_IRQ
.word default_handler // interrupt 34, UART1_IRQ
.word default_handler // interrupt 35, ADC_IRQ_FIFO
.word default_handler // interrupt 36, I2C0_IRQ
.word default_handler // interrupt 37, I2C1_IRQ
.word default_handler // interrupt 38, OTP_IRQ
.word default_handler // interrupt 39, TRNG_IRQ
.word default_handler // interrupt 40, PROC0_IRQ_CTI
.word default_handler // interrupt 41, PROC1_IRQ_CTI
.word default_handler // interrupt 42, PLL_SYS_IRQ
.word default_handler // interrupt 43, PLL_USB_IRQ
.word default_handler // interrupt 44, POWMAN_IRQ_POW
.word default_handler // interrupt 45, POWMAN_IRQ_TIMER
.word default_handler // interrupt 46, SPARE_IRQ_0
.word default_handler // interrupt 47, SPARE_IRQ_1
.word default_handler // interrupt 48, SPARE_IRQ_2
.word default_handler // interrupt 49, SPARE_IRQ_3
.word default_handler // interrupt 50, SPARE_IRQ_4
.word render_irq_handler // interrupt 51, SPARE_IRQ_5, the renderer, in main.sy
