//! Times the 16 bit fill routines in ../fill.sy with the DWT cycle
//! counter at every count from 1 to 640 and both alignments, checks
//! their output, and prints a summary and the full table over USB serial
//! every 10 seconds. A WRONG in the summary, or 65535 in the table, means
//! the fill wrote the wrong pixels.
//!
//! cargo run -r && sleep 1 && screen /dev/tty.usbmodem1101
//!
//! Based on ~/garden/rp2350-measure.

#![no_std]
#![no_main]

use core::fmt::Write;
use core::ptr::{addr_of, addr_of_mut, read_volatile};
use embedded_hal::digital::v2::OutputPin;
use heapless::String;
use panic_halt as _;

use cortex_m::Peripherals as CorePeripherals;
use rp235x_hal::{
    clocks::init_clocks_and_plls, entry, gpio::Pins, pac, sio::Sio, timer::Timer, usb::UsbBus,
    watchdog::Watchdog,
};
use usb_device::{bus::UsbBusAllocator, prelude::*};
use usbd_serial::SerialPort;

/// RP2350 Image Definition block required by the ROM bootloader to boot from Flash
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: rp235x_hal::block::ImageDef = rp235x_hal::block::ImageDef::secure_exe();

// In fill.sy. They take r0-r2, only touch r0-r3 and r12, and return with
// bx lr, so they're compatible with the C calling convention.
extern "C" {
    fn fill_none(dst: *mut u16, color: u32, count: u32);
    fn fill_16(dst: *mut u16, color: u32, count: u32);
}

type FillFn = unsafe extern "C" fn(*mut u16, u32, u32);

const N_VARIANTS: usize = 1;
const VARIANTS: [(&str, FillFn); N_VARIANTS] = [("fill_16", fill_16)];
/// Every count from 1 to MAX_COUNT is measured.
const MAX_COUNT: usize = 640;
/// Ranges summarized by mean cycles: (first, last), inclusive.
const RANGES: [(usize, usize); 5] = [(1, 8), (9, 32), (33, 128), (129, 640), (1, 640)];

const GUARD: usize = 8;
const BUF_LEN: usize = MAX_COUNT + 2 * GUARD + 2;
const BACKGROUND: u16 = 0x5555;
const COLOR: u32 = 0x7c1f;

static mut BUF: [u16; BUF_LEN] = [0; BUF_LEN];
/// Net cycles for [variant][count - 1][align]; u16::MAX means wrong output.
static mut CYCLES: [[[u16; 2]; MAX_COUNT]; N_VARIANTS] = [[[0; 2]; MAX_COUNT]; N_VARIANTS];
static mut REPORT: String<65536> = String::new();

const DWT_CYCCNT: *const u32 = 0xe000_1004 as *const u32;

/// Cycles for one call, the minimum of a few runs. Runs from RAM so flash
/// timing doesn't interfere.
#[link_section = ".data"]
#[inline(never)]
fn time_call(f: FillFn, dst: *mut u16, count: u32) -> u32 {
    let mut best = u32::MAX;
    for _ in 0..4 {
        unsafe {
            let start = read_volatile(DWT_CYCCNT);
            f(dst, COLOR, count);
            let end = read_volatile(DWT_CYCCNT);
            best = best.min(end.wrapping_sub(start));
        }
    }
    best
}

/// Fill the buffer with the background, run f at offset align, and check
/// that exactly count pixels changed. Returns the cycle count, or None if
/// the output is wrong.
fn run(f: FillFn, align: usize, count: u32) -> Option<u32> {
    let buf = unsafe { &mut *addr_of_mut!(BUF) };
    buf.fill(BACKGROUND);
    let start = GUARD + align;
    let dst = unsafe { buf.as_mut_ptr().add(start) };
    let cycles = time_call(f, dst, count);
    let end = start + count as usize;
    for (i, &px) in buf.iter().enumerate() {
        let expected = if (start..end).contains(&i) {
            COLOR as u16
        } else {
            BACKGROUND
        };
        if px != expected {
            return None;
        }
    }
    Some(cycles)
}

fn benchmark() {
    let cycles = unsafe { &mut *addr_of_mut!(CYCLES) };
    for count in 1..=MAX_COUNT {
        for align in 0..2 {
            let base = run(fill_none, align, 0).unwrap_or(0);
            for (v, (_, f)) in VARIANTS.iter().enumerate() {
                cycles[v][count - 1][align] = match run(*f, align, count as u32) {
                    Some(c) => c.saturating_sub(base).min(u16::MAX as u32 - 1) as u16,
                    None => u16::MAX,
                };
            }
        }
    }
}

/// A summary of mean cycles per call over ranges of counts (both
/// alignments), then the full table.
fn write_report(report: &mut String<65536>) {
    let cycles = unsafe { &*addr_of!(CYCLES) };
    let _ = write!(report, "\r\nmean cycles per call, net of call overhead, both alignments\r\n");
    let _ = write!(report, "   counts");
    for (name, _) in VARIANTS {
        let _ = write!(report, " {name:>9}");
    }
    let _ = write!(report, "\r\n");
    for (first, last) in RANGES {
        let _ = write!(report, "{first:>4}-{last:<4}");
        for v in 0..N_VARIANTS {
            let mut sum = 0u32;
            let mut wrong = false;
            for count in first..=last {
                for align in 0..2 {
                    let c = cycles[v][count - 1][align];
                    wrong |= c == u16::MAX;
                    sum += c as u32;
                }
            }
            if wrong {
                let _ = write!(report, " {:>9}", "WRONG");
            } else {
                let mean = sum as f32 / (2 * (last - first + 1)) as f32;
                let _ = write!(report, " {mean:9.1}");
            }
        }
        let _ = write!(report, "\r\n");
    }
    let _ = write!(report, "\r\nnet cycles per call: count, then aligned/unaligned per variant\r\n");
    for count in 1..=MAX_COUNT {
        let _ = write!(report, "{count:3}");
        for v in 0..N_VARIANTS {
            let [a, u] = cycles[v][count - 1];
            let _ = write!(report, " {a:4}/{u:<4}");
        }
        let _ = write!(report, "\r\n");
    }
}

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    let mut core = CorePeripherals::take().unwrap();

    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    const XOSC_CRYSTAL_FREQ: u32 = 12_000_000;
    let clocks = init_clocks_and_plls(
        XOSC_CRYSTAL_FREQ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    let timer = Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);
    let sio = Sio::new(pac.SIO);
    let pins = Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );
    // LED on the Adafruit Feather RP2350
    let mut led = pins.gpio7.into_push_pull_output();

    let usb_bus = UsbBusAllocator::new(UsbBus::new(
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x2e8a, 0x000a))
        .strings(&[usb_device::device::StringDescriptors::default()
            .manufacturer("Raspberry Pi")
            .product("symbolasm fill bench")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    core.DCB.enable_trace();
    core.DWT.enable_cycle_counter();

    // Run once; USB is only polled outside the benchmark, so nothing
    // interrupts it.
    benchmark();
    let report = unsafe { &mut *addr_of_mut!(REPORT) };
    write_report(report);

    let mut led_state = false;
    loop {
        led_state = !led_state;
        let _ = if led_state { led.set_high() } else { led.set_low() };

        let bytes = report.as_bytes();
        let mut written = 0;
        while written < bytes.len() {
            usb_dev.poll(&mut [&mut serial]);
            if let Ok(count) = serial.write(&bytes[written..]) {
                written += count;
            }
        }

        let start = timer.get_counter();
        while timer
            .get_counter()
            .checked_duration_since(start)
            .unwrap()
            .to_millis()
            < 10_000
        {
            usb_dev.poll(&mut [&mut serial]);
        }
    }
}
