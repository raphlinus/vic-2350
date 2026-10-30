//! Interface between user-space and interrupt code.
//!
//! This interface is fairly closely patterned after the USB hardware in
//! the RP2350 chip, but in a more idiomatic Rust style.

use core::{
    cell::UnsafeCell,
    mem::{offset_of, size_of},
    ops::{Deref, DerefMut},
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use crate::pio_usb::host_task::SetupPacket;

const N_PIPES: usize = 16;

#[repr(C)]
pub struct Iface {
    // Bitmask of requests
    request: AtomicU32,
    // Bitmask of responses
    response: AtomicU32,
    // Bitmask of pipes the interrupt retries in the next frame
    held: AtomicU32,
    // Failed transactions: a wrong reply, a bad CRC, and no reply
    errors: [AtomicU32; 3],
    pipes: [UnsafeCell<Pipe>; N_PIPES],
}

#[export_name = "USB_IFACE"]
static IFACE: Iface = Iface::new();

unsafe impl Sync for Iface {}

/// User-space access to shared interface.
pub struct UserIface {
    int_owned: u32,
}

/// Size of the buffers in the ports.
///
/// A value of 64 is sufficient as long as we don't do isochronous transfers.
/// If and when we do implement those, then we probably want to move to an
/// arrangement closer to the RP2350 chip, where there is a buffer space.
const BUF_SIZE: usize = 64;

// Word aligned, so the interrupt side can move buf by words.
#[repr(C, align(4))]
pub struct Pipe {
    pub buf: [u8; BUF_SIZE],
    pub addr: u8,
    pub ep: u8,
    // consider changing this to PID_DATA0 or PID_DATA1, rather than 0 or 1 as now
    pub toggle: u8,
    pub low_speed: bool,
    pub len: u16,
    pub timer: u16,
    pub req: Request,
    pub status: Status,
    /// Consecutive failed transactions, kept by the interrupt side.
    retries: u8,
}

// The interrupt side's view of the interface; these must match its layout.
const _: () = {
    assert!(offset_of!(Iface, request) == 0);
    assert!(offset_of!(Iface, response) == 4);
    assert!(offset_of!(Iface, held) == 8);
    assert!(offset_of!(Iface, errors) == 12);
    assert!(offset_of!(Iface, pipes) == 24);
    assert!(offset_of!(Pipe, buf) == 0);
    assert!(offset_of!(Pipe, addr) == 64);
    assert!(offset_of!(Pipe, ep) == 65);
    assert!(offset_of!(Pipe, toggle) == 66);
    assert!(offset_of!(Pipe, low_speed) == 67);
    assert!(offset_of!(Pipe, len) == 68);
    assert!(offset_of!(Pipe, timer) == 70);
    assert!(offset_of!(Pipe, req) == 72);
    assert!(offset_of!(Pipe, status) == 73);
    assert!(offset_of!(Pipe, retries) == 74);
    assert!(size_of::<Pipe>() == 76);
};

pub struct PipeGuard {
    pipe_ix: usize,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum Request {
    Empty = 0,
    Setup = 1,
    In = 2,
    Out = 3,
    Delay = 4,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Empty = 0,
    Success = 1,
    #[expect(unused, reason = "only set by the interrupt side")]
    Timeout = 2,
    #[expect(unused, reason = "only set by the interrupt side")]
    Error = 4,
    #[expect(unused, reason = "only set by the interrupt side")]
    Stall = 5,
}

impl Pipe {
    const fn new() -> Self {
        Self {
            buf: [0; BUF_SIZE],
            addr: 0,
            ep: 0,
            toggle: 0,
            low_speed: false,
            len: 0,
            timer: 0,
            req: Request::Empty,
            status: Status::Empty,
            retries: 0,
        }
    }

    /// Prepare setup packet
    pub fn setup(&mut self, setup: SetupPacket) {
        self.buf[0..8].copy_from_slice(&setup.raw);
        self.ep = 0;
        self.toggle = 0;
        self.req = Request::Setup;
    }
}

impl Deref for PipeGuard {
    type Target = Pipe;

    fn deref(&self) -> &Self::Target {
        unsafe { &*IFACE.pipes[self.pipe_ix].get() }
    }
}

impl DerefMut for PipeGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *IFACE.pipes[self.pipe_ix].get() }
    }
}

impl Iface {
    const fn new() -> Iface {
        Iface {
            request: AtomicU32::new(0),
            response: AtomicU32::new(0),
            held: AtomicU32::new(0),
            errors: [const { AtomicU32::new(0) }; 3],
            pipes: [const { UnsafeCell::new(Pipe::new()) }; N_PIPES],
        }
    }
}

impl UserIface {
    /// Create a new instance user-space access to the shared interface.
    ///
    /// Safety: there needs to be only one instance; it's bound to `IFACE`
    pub unsafe fn new() -> UserIface {
        UserIface { int_owned: 0 }
    }

    /// Forget all requests and take back all pipes. The interrupt must be
    /// masked.
    pub fn clear(&mut self) {
        IFACE.request.store(0, Relaxed);
        IFACE.response.store(0, Relaxed);
        IFACE.held.store(0, Relaxed);
        for pipe in &IFACE.pipes {
            unsafe { (*pipe.get()).retries = 0 };
        }
        self.int_owned = 0;
    }

    /// Failed transactions: wrong replies, bad CRCs, and no replies.
    pub fn errors(&self) -> [u32; 3] {
        [0, 1, 2].map(|i| IFACE.errors[i].load(Relaxed))
    }

    pub fn pipe(&mut self, pipe_ix: usize) -> PipeGuard {
        assert!(pipe_ix < N_PIPES);
        assert!((self.int_owned >> pipe_ix) & 1 == 0);
        self.int_owned |= 1 << pipe_ix;
        PipeGuard { pipe_ix }
    }

    pub fn release_pipe(&mut self, pipe: PipeGuard) {
        self.int_owned &= !(1 << pipe.pipe_ix);
    }

    pub fn send_req(&mut self, pipe: PipeGuard) {
        IFACE.request.fetch_or(1 << pipe.pipe_ix, Release);
        // possible TODO: trigger interrupt
    }

    pub fn poll(&mut self) -> u32 {
        let mut old = IFACE.response.load(Relaxed);
        loop {
            match IFACE
                .response
                .compare_exchange_weak(old, 0, Acquire, Relaxed)
            {
                Ok(_) => break,
                Err(x) => old = x,
            }
        }
        self.int_owned &= !old;
        old
    }
}

/// Take the set of requests. For the interrupt side, whose language has
/// no exclusives yet.
#[no_mangle]
#[link_section = ".data"]
extern "C" fn usb_iface_poll() -> u32 {
    IFACE.request.swap(0, Acquire)
}

/// Respond on a pipe, returning it to user-space.
#[no_mangle]
#[link_section = ".data"]
extern "C" fn usb_iface_respond(pipe_ix: u32) {
    IFACE.response.fetch_or(1 << pipe_ix, Release);
}
