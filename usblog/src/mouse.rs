//! The mouse, for any core to read.
//!
//! The USB host writes this from core 1, for one mouse. With more than one
//! they add up, and their buttons fight, which is good enough for now.

use core::sync::atomic::{AtomicI32, AtomicU32, Ordering::Relaxed};

static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Where the mouse has moved to since power on, free running, in the
/// mouse's own units, and the buttons held, bit 0 being the left one. The
/// position is a sum, so it's a change between two reads that means something.
#[derive(Clone, Copy)]
pub struct Mouse {
    pub x: i32,
    pub y: i32,
    pub buttons: u32,
}

pub fn read() -> Mouse {
    Mouse {
        x: X.load(Relaxed),
        y: Y.load(Relaxed),
        buttons: BUTTONS.load(Relaxed),
    }
}

/// Take in a boot protocol report: buttons, then x and y movement.
pub(crate) fn report(report: &[u8]) {
    if let [buttons, dx, dy, ..] = *report {
        X.store(X.load(Relaxed).wrapping_add(dx as i8 as i32), Relaxed);
        Y.store(Y.load(Relaxed).wrapping_add(dy as i8 as i32), Relaxed);
        BUTTONS.store(buttons as u32, Relaxed);
    }
}
