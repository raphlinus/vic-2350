//! Cycle timing of the USB host's critical sections, from core 1's DWT
//! cycle counter. Results accumulate from reset, so the worst cases of
//! enumeration stay visible.

use core::ptr::{addr_of, addr_of_mut};

const DEMCR: *mut u32 = 0xe000edfc as *mut u32;
const DWT_CTRL: *mut u32 = 0xe0001000 as *mut u32;
const DWT_CYCCNT: *const u32 = 0xe0001004 as *const u32;

// Bins with two speeds are full speed, then low speed.
pub const TX_TO_RX: usize = 0;
pub const TOKEN_TO_DATA: usize = 2;
pub const EOP_TO_ACK: usize = 4;
pub const RX_POLL: usize = 6;
pub const HANDLER: usize = 8;
/// In microseconds, not cycles.
pub const SOF_LATE: usize = 9;
pub const STAMP: usize = 10;
const N_BINS: usize = 11;

const NAMES: [&str; N_BINS] = [
    "tx_to_rx fs",
    "tx_to_rx ls",
    "token_to_data fs",
    "token_to_data ls",
    "eop_to_ack fs",
    "eop_to_ack ls",
    "rx_poll fs",
    "rx_poll ls",
    "handler",
    "sof_late_us",
    "stamp",
];

#[derive(Clone, Copy)]
struct Bin {
    n: u32,
    min: u32,
    max: u32,
    sum: u64,
}

static mut BINS: [Bin; N_BINS] = [Bin {
    n: 0,
    min: u32::MAX,
    max: 0,
    sum: 0,
}; N_BINS];

#[inline(always)]
pub fn cycles() -> u32 {
    unsafe { DWT_CYCCNT.read_volatile() }
}

/// Start the cycle counter on this core, and measure what a stamp costs.
pub unsafe fn init() {
    DEMCR.write_volatile(DEMCR.read_volatile() | (1 << 24));
    DWT_CTRL.write_volatile(DWT_CTRL.read_volatile() | 1);
    let t0 = cycles();
    let t1 = cycles();
    record(STAMP, t1.wrapping_sub(t0));
}

#[link_section = ".data"]
pub fn record(bin: usize, value: u32) {
    unsafe {
        let b = &mut *addr_of_mut!(BINS[bin]);
        b.n += 1;
        b.min = b.min.min(value);
        b.max = b.max.max(value);
        b.sum += value as u64;
    }
}

/// Print the results. Racing the interrupt only blurs a line.
pub fn report() {
    for (i, name) in NAMES.iter().enumerate() {
        let b = unsafe { addr_of!(BINS[i]).read_volatile() };
        if b.n != 0 {
            console!(
                "{name}: n {} min {} avg {} max {}",
                b.n,
                b.min,
                b.sum / b.n as u64,
                b.max
            );
        }
    }
}
