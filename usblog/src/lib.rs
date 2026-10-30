// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! USB CDC serial logging for vic, linked into the symbolasm program.
//!
//! The program calls `usb_init` once, on core 0, after clk_usb is
//! running at 48 MHz (main.sy's configure_clk_usb) and before interrupts
//! are enabled. It sets up USB and starts
//! core 1, which handles `USBCTRL_IRQ` and drains the `USBLOG` ring buffer
//! to the host, so USB never preempts core 0. The program logs by writing
//! bytes into the ring and bumping `head`, then doing `sev` to wake core 1.
//!
//! To run USB on core 0 instead, call `usb_enable_irq` there in place of
//! starting core 1; the interrupt is at the lowest priority, and setting
//! its pending bit plays the role of `sev`. log.sy's log_flush does both.
//!
//! The critical-section implementation only masks the USB interrupt on
//! the current core, which is enough because all Rust code after
//! `usb_init` runs on the one core that handles USB.
//!
//! Opening the port at 1200 baud and closing it (dropping DTR) reboots
//! into BOOTSEL, so picotool can load a new image without a button press.
//! As a fail-safe, `usb_init` also arms a reboot into BOOTSEL, which is
//! cancelled when the host configures the device; if USB never comes up,
//! the board is back in BOOTSEL after `FAILSAFE_MS`.

#![no_std]

use core::mem::offset_of;
use core::ptr::{addr_of, addr_of_mut};

use cortex_m::asm::{dmb, dsb, isb, wfe};
use cortex_m::register::{basepri, basepri_max};
use rp235x_hal as hal;

use hal::pac;
use hal::usb::UsbBus;
use pac::Interrupt;
use usb_device::class_prelude::UsbBusAllocator;
use usb_device::prelude::*;
use usbd_serial::SerialPort;

/// Print a line to the console ring. Core 1 thread mode only: the ring has
/// one producer, and formatting doesn't belong in the timed interrupt.
macro_rules! console {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::Console, $($arg)*);
    }};
}

mod pio_usb;

pub const LOG_SIZE: usize = 4096;

/// The log ring buffer. The program (producer) writes `buf[head % LOG_SIZE]`
/// and then increments `head`, dropping bytes when `head - tail == LOG_SIZE`.
/// The interrupt (consumer) advances `tail`. Both are free running.
#[repr(C)]
pub struct UsbLog {
    pub head: u32,
    pub tail: u32,
    pub buf: [u8; LOG_SIZE],
}

const _: () = {
    assert!(offset_of!(UsbLog, head) == 0);
    assert!(offset_of!(UsbLog, tail) == 4);
    assert!(offset_of!(UsbLog, buf) == 8);
    assert!(LOG_SIZE.is_power_of_two());
};

#[no_mangle]
pub static mut USBLOG: UsbLog = UsbLog {
    head: 0,
    tail: 0,
    buf: [0; LOG_SIZE],
};

/// Output of the USB host, drained alongside `USBLOG`, which core 0 owns.
static mut CONSOLE: UsbLog = UsbLog {
    head: 0,
    tail: 0,
    buf: [0; LOG_SIZE],
};

const SCREEN_ROWS: usize = 32;
const SCREEN_COLS: usize = 128;

/// The latest lines of console output, for core 0 to draw. Row
/// `lines % SCREEN_ROWS` is being written. Text is printable ASCII, and
/// `len` is published after the bytes it covers, so a reader racing a write
/// sees at worst a torn line.
#[repr(C)]
pub struct Screen {
    pub lines: u32,
    pub len: [u32; SCREEN_ROWS],
    pub text: [[u8; SCREEN_COLS]; SCREEN_ROWS],
}

#[no_mangle]
pub static mut USBSCREEN: Screen = Screen {
    lines: 0,
    len: [0; SCREEN_ROWS],
    text: [[0; SCREEN_COLS]; SCREEN_ROWS],
};

unsafe fn screen_write(s: &str) {
    let screen = addr_of_mut!(USBSCREEN);
    let mut lines = (*screen).lines;
    for b in s.bytes() {
        if b == b'\n' {
            lines = lines.wrapping_add(1);
            addr_of_mut!((*screen).len[lines as usize % SCREEN_ROWS]).write_volatile(0);
            dmb();
            addr_of_mut!((*screen).lines).write_volatile(lines);
        } else {
            let row = lines as usize % SCREEN_ROWS;
            let len = (*screen).len[row] as usize;
            if len < SCREEN_COLS {
                let c = if (32..127).contains(&b) { b } else { b'?' };
                (*screen).text[row][len] = c;
                dmb();
                addr_of_mut!((*screen).len[row]).write_volatile(len as u32 + 1);
            }
        }
    }
}

struct Console;

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe {
            let log = addr_of_mut!(CONSOLE);
            let tail = addr_of!((*log).tail).read_volatile();
            let mut head = (*log).head;
            for b in s.bytes() {
                if head.wrapping_sub(tail) as usize == LOG_SIZE {
                    break;
                }
                (*log).buf[head as usize & (LOG_SIZE - 1)] = b;
                head = head.wrapping_add(1);
            }
            dmb();
            addr_of_mut!((*log).head).write_volatile(head);
            screen_write(s);
        }
        Ok(())
    }
}

const FAILSAFE_MS: u32 = 8000;
/// The bootrom's `reboot` flag for a reboot into BOOTSEL. Unlike
/// `hal::reboot::reboot`, which always waits 500 ms and doesn't return,
/// calling the ROM directly lets us choose the delay and carry on.
const REBOOT_TYPE_BOOTSEL: u32 = 0x0002;
/// clk_ref in MHz, so the watchdog ticks at 1 MHz.
const CLK_REF_MHZ: u16 = 12;

/// The VID:PID of the Pico SDK's USB serial, so host tools recognize it.
const USB_VID_PID: UsbVidPid = UsbVidPid(0x2e8a, 0x000a);

/// The lowest priority. RP2350 implements the top 4 bits, so this reads
/// back as 0xf0.
const USB_PRIORITY: u8 = 0xff;

static mut USB_BUS: Option<UsbBusAllocator<UsbBus>> = None;
static mut USB: Option<Usb> = None;
static mut CORE1_STACK: hal::multicore::Stack<4096> = hal::multicore::Stack::new();
static mut FAILSAFE_ARMED: bool = false;

struct Usb {
    dev: UsbDevice<'static, UsbBus>,
    serial: SerialPort<'static, UsbBus>,
}

/// Rust code runs in `usb_init` (before interrupts are enabled) and then
/// only on the core handling USB, so a critical section only needs to mask
/// the USB interrupt there. Masking with BASEPRI rather than PRIMASK keeps
/// higher priority interrupts, like the DVI DMA one if USB is on core 0,
/// on time.
struct BasepriCs;
critical_section::set_impl!(BasepriCs);

unsafe impl critical_section::Impl for BasepriCs {
    unsafe fn acquire() -> u8 {
        let old = basepri::read();
        // BASEPRI_MAX only raises the mask, so nesting works.
        basepri_max::write(USB_PRIORITY);
        old
    }

    unsafe fn release(old: u8) {
        basepri::write(old);
    }
}

/// Enable the FPU on this core, in case the compiler emits floating point
/// instructions.
unsafe fn enable_fpu() {
    cortex_m::Peripherals::steal().SCB.enable_fpu();
    dsb();
    isb();
}

/// Make exclusive loads and stores on this core use the global monitor, as
/// the default memory map isn't shareable, and without this atomics don't
/// synchronize between cores. Needs doing on each core. The global monitor
/// doesn't cover PSRAM, so atomics there won't work.
unsafe fn enable_global_exclusives() {
    const ACTLR: *mut u32 = 0xe000e008 as *mut u32;
    const EXTEXCLALL: u32 = 1 << 29;
    ACTLR.write_volatile(ACTLR.read_volatile() | EXTEXCLALL);
    dsb();
    isb();
}

/// Set up USB CDC serial and start core 1 to run it. Call once, on core 0,
/// with interrupts still disabled.
#[no_mangle]
pub unsafe extern "C" fn usb_init() {
    enable_fpu();
    enable_global_exclusives();

    let mut p = pac::Peripherals::steal();
    // The watchdog ticks at 1 MHz, then arm the fail-safe.
    let tick = p.TICKS.tickwatchdog();
    tick.cycles().write(|w| w.proc0_cycles().bits(CLK_REF_MHZ));
    tick.ctrl().write(|w| w.enable().set_bit());
    hal::rom_data::reboot(REBOOT_TYPE_BOOTSEL, FAILSAFE_MS, 0, 0);
    *addr_of_mut!(FAILSAFE_ARMED) = true;

    let clocks = hal::clocks::ClocksManager::new(pac::Peripherals::steal().CLOCKS);
    let bus = UsbBus::new(p.USB, p.USB_DPRAM, clocks.usb_clock, true, &mut p.RESETS);
    let alloc: &'static UsbBusAllocator<UsbBus> =
        (*addr_of_mut!(USB_BUS)).insert(UsbBusAllocator::new(bus));
    let serial = SerialPort::new(alloc);
    let dev = UsbDeviceBuilder::new(alloc, USB_VID_PID)
        .strings(&[StringDescriptors::default()
            .manufacturer("Raspberry Pi")
            .product("vic-2350")
            .serial_number("vic")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();
    *addr_of_mut!(USB) = Some(Usb { dev, serial });

    // Timer 0 ticks at 1 MHz, for the USB host.
    let tick = p.TICKS.ticktimer0();
    tick.cycles().write(|w| w.proc0_cycles().bits(CLK_REF_MHZ));
    tick.ctrl().write(|w| w.enable().set_bit());
    p.RESETS.reset().modify(|_, w| w.timer0().clear_bit());
    while p.RESETS.reset_done().read().timer0().bit_is_clear() {}
    pio_usb::init(p.PIO0, &mut p.RESETS);

    // All the setup that touches shared registers, like RESETS, is done,
    // so core 1 doesn't race core 0's startup.
    let mut sio = hal::Sio::new(p.SIO);
    let mut mc = hal::multicore::Multicore::new(&mut p.PSM, &mut p.PPB, &mut sio.fifo);
    let stack = &mut (*addr_of_mut!(CORE1_STACK)).mem;
    let _ = mc.cores()[1].spawn(stack, || unsafe { core1_main() });
}

/// Enable `USBCTRL_IRQ` on the calling core, at the lowest priority.
#[no_mangle]
pub unsafe extern "C" fn usb_enable_irq() {
    let mut nvic = cortex_m::Peripherals::steal().NVIC;
    nvic.set_priority(Interrupt::USBCTRL_IRQ, USB_PRIORITY);
    cortex_m::peripheral::NVIC::unmask(Interrupt::USBCTRL_IRQ);
}

unsafe fn core1_main() {
    enable_fpu();
    enable_global_exclusives();
    usb_enable_irq();
    let mut usb_task = pio_usb::UsbTask::new();
    pio_usb::start();
    loop {
        // Woken by an interrupt, or by sev from a logging core 0.
        wfe();
        usb_task.poll();
        critical_section::with(|_| {
            if let Some(usb) = (*addr_of_mut!(USB)).as_mut() {
                if usb.dev.state() == UsbDeviceState::Configured && usb.serial.dtr() {
                    drain(&mut usb.serial);
                }
            }
        });
    }
}

#[no_mangle]
pub unsafe extern "C" fn USBCTRL_IRQ() {
    let Some(usb) = (*addr_of_mut!(USB)).as_mut() else {
        return;
    };
    if usb.dev.poll(&mut [&mut usb.serial]) {
        // Discard anything the host sends.
        let mut scratch = [0u8; 64];
        while let Ok(n) = usb.serial.read(&mut scratch) {
            if n == 0 {
                break;
            }
        }
    }
    if usb.serial.line_coding().data_rate() == 1200 && !usb.serial.dtr() {
        hal::reboot::reboot(
            hal::reboot::RebootKind::BootSel {
                picoboot_disabled: false,
                msd_disabled: false,
            },
            hal::reboot::RebootArch::Normal,
        );
    }
    if usb.dev.state() == UsbDeviceState::Configured {
        if *addr_of!(FAILSAFE_ARMED) {
            // USB works: cancel the fail-safe reboot by stopping the
            // watchdog, and clearing the ROM's reboot request from scratch4.
            let watchdog = &pac::Peripherals::steal().WATCHDOG;
            watchdog.ctrl().modify(|_, w| w.enable().clear_bit());
            watchdog.scratch4().write(|w| unsafe { w.bits(0) });
            *addr_of_mut!(FAILSAFE_ARMED) = false;
        }
        // Hold the log until a program on the host has the port open.
        if usb.serial.dtr() {
            drain(&mut usb.serial);
        }
    }
}

unsafe fn drain(serial: &mut SerialPort<'static, UsbBus>) {
    drain_ring(serial, addr_of_mut!(USBLOG));
    drain_ring(serial, addr_of_mut!(CONSOLE));
}

unsafe fn drain_ring(serial: &mut SerialPort<'static, UsbBus>, log: *mut UsbLog) {
    let buf = addr_of!((*log).buf) as *const u8;
    let mut tail = addr_of!((*log).tail).read_volatile();
    loop {
        let head = addr_of!((*log).head).read_volatile();
        if head == tail {
            break;
        }
        // Pairs with the producer's dmb between writing bytes and head.
        dmb();
        let start = tail as usize & (LOG_SIZE - 1);
        let len = ((head - tail) as usize).min(LOG_SIZE - start);
        let chunk = core::slice::from_raw_parts(buf.add(start), len);
        match serial.write(chunk) {
            Ok(n) if n > 0 => {
                tail = tail.wrapping_add(n as u32);
                // The bytes are copied out before the producer can reuse them.
                dmb();
                addr_of_mut!((*log).tail).write_volatile(tail);
            }
            _ => break,
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    console!("{info}");
    loop {}
}
