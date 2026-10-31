// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Decodes an mp3 file with ../mp3.sy and with the mp3dec crate it's a
//! port of, timing each frame with the DWT cycle counter, and checks both
//! against a hash of the samples mp3dec gives on the host. When the host
//! opens the serial port the results are printed, ending with #END:
//!
//! MP3=song.mp3 cargo build -r && ../tools/run.sh target/thumbv8m.main-none-eabihf/release/mp3bench 30
//!
//! (run.sh passes picotool -t elf, as the file has no extension.)
//!
//! The file, $MP3 at build time, is built into the image, so it can be up
//! to about 1.5 MB. mp3dec is expected next to this repo, as symbolasm is.
//! Opening the port at 1200 baud and closing it reboots into BOOTSEL.
//!
//! The decoding runs at boot, before USB is up, under the watchdog, with
//! its progress in a watchdog scratch register. A fault, panic or hang
//! ends in a watchdog reboot, and then what happened, and where, is
//! reported instead, so the board never needs its BOOTSEL button.
//!
//! Based on rp2350-measure.

#![no_std]
#![no_main]

use core::fmt::Write;
use core::ptr::{addr_of, addr_of_mut, read_volatile};
use cortex_m_rt::{exception, ExceptionFrame};
use heapless::String;

use cortex_m::Peripherals as CorePeripherals;
use rp235x_hal::{
    self as hal,
    clocks::init_clocks_and_plls,
    entry,
    fugit::ExtU32,
    pac,
    usb::UsbBus,
    watchdog::{ScratchRegister, Watchdog},
};
use usb_device::{bus::UsbBusAllocator, prelude::*};
use usbd_serial::SerialPort;

/// RP2350 Image Definition block required by the ROM bootloader to boot from Flash
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: rp235x_hal::block::ImageDef = rp235x_hal::block::ImageDef::secure_exe();

include!(concat!(env!("OUT_DIR"), "/expected.rs"));

static INPUT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/input.mp3"));

/// Mp3Info in mp3.sy
#[repr(C)]
#[derive(Default)]
struct Mp3Info {
    frame_bytes: u32,
    frame_offset: u32,
    channels: u32,
    hz: u32,
    layer: u32,
    bitrate_kbps: u32,
}

// In mp3.sy, which follows the AAPCS with hard float.
extern "C" {
    fn mp3_init();
    fn mp3_decode_frame(mp3: *const u8, len: u32, pcm: *mut i16, info: *mut Mp3Info) -> u32;
}

static mut PCM: [i16; mp3dec::MAX_SAMPLES_PER_FRAME] = [0; mp3dec::MAX_SAMPLES_PER_FRAME];

const DWT_CYCCNT: *const u32 = 0xe000_1004 as *const u32;
const CFSR: *const u32 = 0xe000_ed28 as *const u32;
const BFAR: *const u32 = 0xe000_ed38 as *const u32;

// SCRATCH0 while decoding: RUNNING, the decoder and the frame. FAULT or
// PANIC is added when one happens, and for a fault SCRATCH1-3 hold the pc,
// CFSR and BFAR.
const RUNNING: u32 = 0xa500_0000;
const RUNNING_MASK: u32 = 0xff00_0000;
const RUST: u32 = 1 << 23;
const FAULT: u32 = 1 << 22;
const PANIC: u32 = 1 << 21;
const FRAME_MASK: u32 = 0xf_ffff;

fn scratch() -> &'static pac::watchdog::RegisterBlock {
    unsafe { &*pac::WATCHDOG::ptr() }
}

/// Note where decoding is, for after a watchdog reboot
fn progress(decoder: u32, frame: u32) {
    scratch()
        .scratch0()
        .write(|w| unsafe { w.bits(RUNNING | decoder | frame) });
}

/// Record the fault, and wait for the watchdog
#[exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    let s = scratch();
    s.scratch1().write(|w| w.bits(ef.pc()));
    s.scratch2().write(|w| w.bits(read_volatile(CFSR)));
    s.scratch3().write(|w| w.bits(read_volatile(BFAR)));
    s.scratch0().modify(|r, w| w.bits(r.bits() | FAULT));
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    scratch()
        .scratch0()
        .modify(|r, w| unsafe { w.bits(r.bits() | PANIC) });
    loop {}
}
/// clk_sys, as init_clocks_and_plls sets it
const CLOCK_HZ: u64 = 150_000_000;

/// FNV-1a over the samples of each frame; must match build.rs
fn hash(h: u32, samples: &[i16]) -> u32 {
    samples.iter().fold(h, |h, s| {
        let h = (h ^ (*s as u16 as u32 & 0xff)).wrapping_mul(0x0100_0193);
        (h ^ (*s as u16 as u32 >> 8)).wrapping_mul(0x0100_0193)
    })
}

#[derive(Default)]
struct Stats {
    frames: u32,
    /// Frames with samples
    decoded: u32,
    cycles: u64,
    max_cycles: u32,
    hash: u32,
    /// Seconds of audio
    seconds: f32,
}

impl Stats {
    fn add(&mut self, n: usize, channels: usize, hz: u32, cycles: u32, pcm: &[i16]) {
        self.frames += 1;
        if n > 0 {
            self.decoded += 1;
            self.cycles += cycles as u64;
            self.max_cycles = self.max_cycles.max(cycles);
            self.seconds += n as f32 / hz as f32;
        }
        self.hash = hash(self.hash, &pcm[..n * channels]);
    }

    fn report(&self, name: &str, out: &mut String<1024>) {
        let mean = self.cycles.checked_div(self.decoded as u64).unwrap_or(0);
        // The share of a core decoding takes, playing in real time
        let load = self.cycles as f32 / (self.seconds * CLOCK_HZ as f32);
        let _ = write!(
            out,
            "{name}: {} frames, {} decoded, {mean} cycles per frame (max {}), {:.1}% of a core at {} MHz, ",
            self.frames,
            self.decoded,
            self.max_cycles,
            100.0 * load,
            CLOCK_HZ / 1_000_000,
        );
        if self.hash == EXPECTED {
            let _ = writeln!(out, "samples match\r");
        } else {
            let _ = writeln!(
                out,
                "samples WRONG: hash {:#010x}, want {EXPECTED:#010x}\r",
                self.hash
            );
        }
    }
}

fn bench_sy(watchdog: &Watchdog) -> Stats {
    let pcm = unsafe { &mut *addr_of_mut!(PCM) };
    let mut stats = Stats {
        hash: 0x811c_9dc5,
        ..Default::default()
    };
    let mut info = Mp3Info::default();
    unsafe { mp3_init() };
    let mut offset = 0;
    while offset < INPUT.len() {
        watchdog.feed();
        progress(0, stats.frames);
        let rest = &INPUT[offset..];
        let (n, cycles) = unsafe {
            let start = read_volatile(DWT_CYCCNT);
            let n = mp3_decode_frame(
                rest.as_ptr(),
                rest.len() as u32,
                pcm.as_mut_ptr(),
                &mut info,
            );
            (n, read_volatile(DWT_CYCCNT).wrapping_sub(start))
        };
        stats.add(n as usize, info.channels as usize, info.hz, cycles, pcm);
        if info.frame_bytes == 0 {
            break;
        }
        offset += info.frame_bytes as usize;
    }
    stats
}

fn bench_rust(watchdog: &Watchdog) -> Stats {
    let pcm = unsafe { &mut *addr_of_mut!(PCM) };
    let mut stats = Stats {
        hash: 0x811c_9dc5,
        ..Default::default()
    };
    let mut decoder = mp3dec::Decoder::new();
    let mut info = mp3dec::FrameInfo::default();
    let mut offset = 0;
    while offset < INPUT.len() {
        watchdog.feed();
        progress(RUST, stats.frames);
        let rest = &INPUT[offset..];
        let (n, cycles) = unsafe {
            let start = read_volatile(DWT_CYCCNT);
            let n = decoder.decode_frame(rest, Some(&mut *pcm), &mut info);
            (n, read_volatile(DWT_CYCCNT).wrapping_sub(start))
        };
        stats.add(n, info.channels, info.hz as u32, cycles, pcm);
        if info.frame_bytes == 0 {
            break;
        }
        offset += info.frame_bytes;
    }
    stats
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

    core.DCB.enable_trace();
    core.DWT.enable_cycle_counter();

    // mp3dec's code into RAM; see memory.x
    extern "C" {
        static mut __smp3dec_text: u32;
        static __emp3dec_text: u32;
        static __simp3dec_text: u32;
    }
    unsafe {
        let dst = addr_of_mut!(__smp3dec_text);
        let words = (addr_of!(__emp3dec_text) as usize - dst as usize) / 4;
        core::ptr::copy_nonoverlapping(addr_of!(__simp3dec_text), dst, words);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }

    let mut report: String<1024> = String::new();
    let last = watchdog.read_scratch(ScratchRegister::Scratch0);
    watchdog.write_scratch(ScratchRegister::Scratch0, 0);
    if last & RUNNING_MASK == RUNNING {
        // The last run didn't finish
        let decoder = if last & RUST != 0 { "mp3dec" } else { "mp3.sy" };
        let what = if last & FAULT != 0 {
            "faulted"
        } else if last & PANIC != 0 {
            "panicked"
        } else {
            "hung"
        };
        let _ = write!(
            report,
            "{decoder} {what} decoding frame {}",
            last & FRAME_MASK
        );
        if last & FAULT != 0 {
            let _ = write!(
                report,
                ": pc {:#010x}, CFSR {:#010x}, BFAR {:#010x}",
                watchdog.read_scratch(ScratchRegister::Scratch1),
                watchdog.read_scratch(ScratchRegister::Scratch2),
                watchdog.read_scratch(ScratchRegister::Scratch3),
            );
        }
        let _ = writeln!(report, "\r");
    } else {
        // Before USB is up, so nothing interrupts it
        let _ = writeln!(report, "{} bytes of input\r", INPUT.len());
        watchdog.start(500.millis());
        // rp235x-hal's start resets only what it would on the RP2040
        // (0x1ffff), which on the RP2350 leaves out the cores, so the
        // timeout would stop the clocks and XIP under a running core.
        // Reset everything but the oscillators, as pico-sdk does.
        unsafe {
            (*pac::PSM::ptr())
                .wdsel()
                .write(|w| w.bits(0x01ff_ffff & !(1 << 2 | 1 << 3)));
        }
        bench_sy(&watchdog).report("mp3.sy", &mut report);
        bench_rust(&watchdog).report("mp3dec", &mut report);
        watchdog.disable();
        watchdog.write_scratch(ScratchRegister::Scratch0, 0);
    }
    let _ = writeln!(report, "#END");

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
            .product("symbolasm mp3 bench")
            .serial_number("mp3bench")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let mut sent = false;
    let mut scratch = [0u8; 64];
    loop {
        usb_dev.poll(&mut [&mut serial]);
        while let Ok(n) = serial.read(&mut scratch) {
            if n == 0 {
                break;
            }
        }
        if serial.line_coding().data_rate() == 1200 && !serial.dtr() {
            hal::reboot::reboot(
                hal::reboot::RebootKind::BootSel {
                    picoboot_disabled: false,
                    msd_disabled: false,
                },
                hal::reboot::RebootArch::Normal,
            );
        }
        let open = usb_dev.state() == UsbDeviceState::Configured && serial.dtr();
        if open && !sent {
            sent = true;
            let bytes = report.as_bytes();
            let mut written = 0;
            while written < bytes.len() && serial.dtr() {
                usb_dev.poll(&mut [&mut serial]);
                if let Ok(n) = serial.write(&bytes[written..]) {
                    written += n;
                }
            }
        } else if !open {
            sent = false;
        }
    }
}
