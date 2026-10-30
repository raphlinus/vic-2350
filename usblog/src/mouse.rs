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

/// A field of a report, in bits, not counting the report ID.
#[derive(Clone, Copy, Debug)]
struct Field {
    start: u16,
    size: u8,
}

/// Where a mouse's reports keep things.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    /// The first byte of the reports to read, if they have one.
    id: Option<u8>,
    buttons: Field,
    x: Field,
    y: Field,
}

impl Field {
    /// Read this from little-endian report bits. Bits past the end are 0.
    fn read(self, report: &[u8], skip: u16) -> u32 {
        let start = (self.start + skip) as usize;
        let mut value = 0;
        for i in 0..(self.size as usize).min(32) {
            let bit = start + i;
            let byte = report.get(bit / 8).copied().unwrap_or(0);
            value |= (byte as u32 >> (bit % 8) & 1) << i;
        }
        value
    }

    fn read_signed(self, report: &[u8], skip: u16) -> i32 {
        let shift = 32 - (self.size as u32).clamp(1, 32);
        ((self.read(report, skip) << shift) as i32) >> shift
    }
}

impl Layout {
    /// Buttons, then x and y movement.
    pub(crate) const BOOT: Self = Self {
        id: None,
        buttons: Field { start: 0, size: 8 },
        x: Field { start: 8, size: 8 },
        y: Field { start: 16, size: 8 },
    };

    /// Find the first report with relative x and y in a HID report
    /// descriptor. Push and pop aren't handled.
    pub(crate) fn parse(desc: &[u8]) -> Option<Self> {
        const GENERIC_DESKTOP: u32 = 1;
        const BUTTON_PAGE: u32 = 9;
        const USAGE_X: u32 = 0x30;
        const USAGE_Y: u32 = 0x31;
        const CONSTANT: u32 = 1 << 0;
        const VARIABLE: u32 = 1 << 1;
        const RELATIVE: u32 = 1 << 2;
        const MAX_USAGES: usize = 16;

        let mut page = 0;
        let (mut report_size, mut report_count) = (0, 0);
        let mut id = None;
        // The bits so far of each report
        let mut offsets = [0; 256];
        let mut usages = [0; MAX_USAGES];
        let mut n_usages = 0;
        let (mut usage_min, mut usage_max) = (None, 0);
        let mut buttons = None;
        let (mut x, mut y) = (None, None);

        let mut rest = desc;
        while let [prefix, tail @ ..] = rest {
            if *prefix == 0xfe {
                // A long item, with its size in the next byte
                rest = tail.get(2 + *tail.first()? as usize..)?;
                continue;
            }
            let size = [0, 1, 2, 4][*prefix as usize & 3];
            let (bytes, tail) = tail.split_at_checked(size)?;
            rest = tail;
            let data = bytes.iter().rev().fold(0, |v, b| v << 8 | *b as u32);
            let tag = prefix >> 4;
            match (prefix >> 2) & 3 {
                1 => match tag {
                    0 => page = data,
                    7 => report_size = data,
                    8 => {
                        if let (Some(x), Some(y)) = (x, y) {
                            let buttons = buttons?;
                            return Some(Self { id, buttons, x, y });
                        }
                        id = Some(data as u8);
                        buttons = None;
                    }
                    9 => report_count = data,
                    _ => (),
                },
                2 => match tag {
                    0 => {
                        if n_usages < MAX_USAGES {
                            usages[n_usages] = data;
                            n_usages += 1;
                        }
                    }
                    1 => usage_min = Some(data),
                    2 => usage_max = data,
                    _ => (),
                },
                0 => {
                    if tag == 8 {
                        let offset = &mut offsets[id.unwrap_or(0) as usize];
                        for i in 0..report_count {
                            let usage = match usage_min {
                                Some(min) if min + i <= usage_max => min + i,
                                Some(_) => 0,
                                None if n_usages == 0 => 0,
                                None => usages[(i as usize).min(n_usages - 1)],
                            };
                            // An extended usage has its page in the top half.
                            let (field_page, usage) = match usage >> 16 {
                                0 => (page, usage),
                                high => (high, usage & 0xffff),
                            };
                            let field = Field {
                                start: *offset + (i * report_size) as u16,
                                size: report_size as u8,
                            };
                            if data & (CONSTANT | VARIABLE) != VARIABLE {
                                continue;
                            }
                            match (field_page, usage) {
                                (GENERIC_DESKTOP, USAGE_X) if data & RELATIVE != 0 => {
                                    x = x.or(Some(field));
                                }
                                (GENERIC_DESKTOP, USAGE_Y) if data & RELATIVE != 0 => {
                                    y = y.or(Some(field));
                                }
                                (BUTTON_PAGE, 1) => {
                                    buttons = buttons.or(Some(Field {
                                        size: report_count.min(32) as u8,
                                        ..field
                                    }));
                                }
                                _ => (),
                            }
                        }
                        *offset += (report_count * report_size) as u16;
                    }
                    n_usages = 0;
                    usage_min = None;
                }
                _ => (),
            }
        }
        Some(Self {
            id,
            buttons: buttons?,
            x: x?,
            y: y?,
        })
    }
}

/// Take in a report.
pub(crate) fn report(report: &[u8], layout: &Layout) {
    let skip = match layout.id {
        Some(id) if report.first() != Some(&id) => return,
        Some(_) => 8,
        None => 0,
    };
    let dx = layout.x.read_signed(report, skip);
    let dy = layout.y.read_signed(report, skip);
    X.store(X.load(Relaxed).wrapping_add(dx), Relaxed);
    Y.store(Y.load(Relaxed).wrapping_add(dy), Relaxed);
    BUTTONS.store(layout.buttons.read(report, skip), Relaxed);
}
