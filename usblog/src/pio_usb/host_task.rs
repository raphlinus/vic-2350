//! A task that is continuously polled to run the USB host role.
//!
//! The root port has a hub on it, which is enumerated on pipe 0, and then
//! polled there for status changes. Pipe 1 does everything else that's
//! control transfers, for one port at a time: changes on the ports, and
//! enumerating what's attached to them. An enumerated device gets a pipe
//! of its own to poll.
//!
//! A failed transfer is recovered from at the smallest scale that works.
//! A device is reset by its port, a few times, and a failure of the hub
//! restarts the bus. The hub's status change reports are level triggered,
//! so a port can always be gone back to.

use crate::{
    mouse,
    pio_usb::{
        iface::{PipeGuard, Request, Status, UserIface},
        restart,
        wire::{
            BOOT_SUBCLASS, CLASS_REQUEST, CLEAR_FEATURE, CONFIG_DESCRIPTOR, DEVICE_DESCRIPTOR,
            DEVICE_TO_HOST, ENDPOINT_DESCRIPTOR, GET_DESCRIPTOR, GET_STATUS, HID_CLASS,
            HID_DESCRIPTOR, HOST_TO_DEVICE, HUB_DESCRIPTOR, INTERFACE_DESCRIPTOR,
            KEYBOARD_PROTOCOL, MOUSE_PROTOCOL, PORT_POWER, PORT_RESET, RECIPIENT_INTERFACE,
            RECIPIENT_OTHER, REPORT_DESCRIPTOR, SET_ADDRESS, SET_CONFIGURATION, SET_FEATURE,
            SET_PROTOCOL,
        },
    },
};

/// Maximum number of pipes that will be allocated
const N_PIPES: usize = 16;

const HUB_PIPE: usize = 0;
const CTRL_PIPE: usize = 1;

const HUB_ADDR: u8 = 1;
/// As many as fit in the first byte of the hub's status change report.
const MAX_PORTS: usize = 7;

/// How many frames of NAKs a control transfer sits through.
const NAK_FRAMES: u16 = 1000;
/// As much of a descriptor as is kept. A mouse's report descriptor can run
/// to a couple of hundred bytes.
const TRANSFER_BUF: usize = 256;
/// Resets of a port before giving up on what's attached to it.
const ENUM_ATTEMPTS: u8 = 3;
/// Checks of a port that's slow to finish its reset.
const RESET_POLLS: u8 = 10;

// In frames. TRSTRCY is 10 ms, and SET_ADDRESS gets 2 ms.
const RESET_RECOVERY: u16 = 20;
const ADDRESS_RECOVERY: u16 = 10;
const CONNECT_DEBOUNCE: u16 = 100;
const RESET_WAIT: u16 = 20;

// wPortStatus
const PORT_CONNECTED: u16 = 1 << 0;
const PORT_ENABLED: u16 = 1 << 1;
const PORT_RESETTING: u16 = 1 << 4;
const PORT_LOW_SPEED: u16 = 1 << 9;
// wPortChange, without bits that aren't defined
const PORT_CHANGES: u16 = 0x1f;

/// Print progress, which the soak test has too much of.
macro_rules! trace {
    ($($arg:tt)*) => {
        if !SOAK {
            console!($($arg)*);
        }
    };
}

/// Run the soak test: reset and enumerate, for as long as the power is on.
/// Set `USB_SOAK` when building.
const SOAK: bool = option_env!("USB_SOAK").is_some();
const SOAK_DEADLINE_US: u32 = 5_000_000;
const SOAK_SETTLE_US: u32 = 100_000;
const N_MODES: usize = 3;

/// A task encapsulating the host side of a USB bus.
///
/// It's conceptually similar to an async task, but there is no async
/// involved here. Rather, tasks are implemented as explicit state
/// machines.
pub struct UsbTask {
    iface: UserIface,
    transfers: [Transfer; N_PIPES],
    hub: HubState,
    n_ports: u8,
    /// Ports to look at, by bit.
    dirty: u16,
    ctrl: CtrlState,
    /// The port pipe 1 is working on, and its latest wPortStatus.
    port: u16,
    port_status: u16,
    reset_polls: u8,
    ports: [Port; MAX_PORTS + 1],
    restart: Option<Restart>,
    /// Bus restarts and enumeration failures, since power on.
    bus_restarts: u32,
    enum_failures: u32,
    soak: Soak,
}

#[derive(Copy, Clone)]
pub struct SetupPacket {
    pub raw: [u8; 8],
}

struct Transfer {
    ty: TransferType,
    phase: TransferPhase,
    buf: [u8; TRANSFER_BUF],
    packet_size: u16,
    target_len: u16,
    actual_len: u16,
    pipe: Option<PipeGuard>,
}

enum TransferType {
    ControlNone,
    ControlIn,
    PollInterrupt,
}

#[derive(Clone, Copy, Debug)]
enum TransferPhase {
    Idle,
    Setup,
    Data,
    Status,
    Delay,
}

#[derive(Clone, Copy, PartialEq)]
enum TickResult {
    Send,
    Done,
    Error,
}

#[derive(Clone, Copy, Debug)]
enum HubState {
    Recovery,
    Desc,
    Address,
    AddressDelay,
    Configured,
    HubDesc,
    PowerPort(u8),
    PowerGood,
    Polling,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum CtrlState {
    Idle,
    PortStatus,
    ClearedChange,
    Debounce,
    DidReset,
    ResetWait,
    ResetRecovery,
    GotDesc,
    Address,
    AddressDelay,
    GotConfig,
    Configured,
    GotReportDesc,
    ProtocolSet,
}

#[derive(Clone, Copy)]
struct Port {
    phase: PortPhase,
    attempts: u8,
    low_speed: bool,
    interface: Interface,
    /// How its reports are laid out, if it's a mouse.
    layout: mouse::Layout,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Mouse,
    Keyboard,
    Other,
}

/// The interface of a device to use.
#[derive(Clone, Copy, Debug)]
struct Interface {
    kind: Kind,
    number: u8,
    /// Its interrupt IN endpoint.
    ep: u8,
    /// The length of its report descriptor.
    report_len: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PortPhase {
    /// Nothing is known to be attached.
    Empty,
    /// Reset, and enumerating.
    Enumerating,
    /// The device is being polled.
    Running,
    /// Given up on, until the next connection.
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
enum Restart {
    Bus,
    Power,
}

/// Outcomes of the soak test, by what was reset, with counts of what it
/// takes for the device to deliver a report.
struct Soak {
    mode: usize,
    start: u32,
    /// When the report came, while settling.
    settling: Option<u32>,
    reported: bool,
    ok: [u32; N_MODES],
    failed: [u32; N_MODES],
    max_ms: [u32; N_MODES],
}

const MODE_NAMES: [&str; N_MODES] = ["port", "bus", "power"];

impl SetupPacket {
    #[link_section = ".data"]
    pub fn new(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> Self {
        let mut raw = [0; 8];
        raw[0] = request_type;
        raw[1] = request;
        raw[2..4].copy_from_slice(&u16::to_le_bytes(value));
        raw[4..6].copy_from_slice(&u16::to_le_bytes(index));
        raw[6..8].copy_from_slice(&u16::to_le_bytes(length));
        SetupPacket { raw }
    }

    pub fn length(&self) -> u16 {
        u16::from_le_bytes(self.raw[6..8].try_into().unwrap())
    }
}

impl Interface {
    /// What's assumed of a device with no HID boot interface.
    const fn new() -> Self {
        Self {
            kind: Kind::Other,
            number: 0,
            ep: 1,
            report_len: 0,
        }
    }

    /// Pick the interface to use from a configuration descriptor, which may
    /// be cut short. A mouse is preferred, so a receiver with both on one
    /// address works as a mouse, and it's a boot one so its reports don't
    /// need parsing against a report descriptor.
    fn parse(config: &[u8]) -> Self {
        let mut best = Self::new();
        let mut current = Self::new();
        let mut rest = config;
        while let &[len, ty, ..] = rest {
            let Some((desc, tail)) = rest.split_at_checked(len as usize) else {
                break;
            };
            if len < 2 {
                break;
            }
            rest = tail;
            match (ty, desc) {
                (INTERFACE_DESCRIPTOR, &[_, _, number, _, _, class, subclass, protocol, ..]) => {
                    let kind = match (class, subclass, protocol) {
                        (HID_CLASS, BOOT_SUBCLASS, MOUSE_PROTOCOL) => Kind::Mouse,
                        (HID_CLASS, BOOT_SUBCLASS, KEYBOARD_PROTOCOL) => Kind::Keyboard,
                        _ => Kind::Other,
                    };
                    current = Self {
                        kind,
                        number,
                        ep: 0,
                        report_len: 0,
                    };
                }
                (HID_DESCRIPTOR, &[_, _, _, _, _, _, REPORT_DESCRIPTOR, lo, hi, ..]) => {
                    current.report_len = u16::from_le_bytes([lo, hi]);
                }
                (ENDPOINT_DESCRIPTOR, &[_, _, addr, attributes, ..]) => {
                    // The first interrupt IN endpoint of a boot interface
                    if addr & 0x80 != 0 && attributes & 3 == 3 && current.ep == 0 {
                        current.ep = addr & 0xf;
                        let better = match (current.kind, best.kind) {
                            (Kind::Other, _) | (_, Kind::Mouse) => false,
                            (Kind::Keyboard, Kind::Keyboard) => false,
                            _ => true,
                        };
                        if better {
                            best = current;
                        }
                    }
                }
                _ => (),
            }
        }
        best
    }
}

impl Transfer {
    const fn new() -> Self {
        Self {
            ty: TransferType::ControlNone,
            phase: TransferPhase::Idle,
            buf: [0; TRANSFER_BUF],
            packet_size: 0,
            target_len: 0,
            actual_len: 0,
            pipe: None,
        }
    }

    fn tick(&mut self) -> TickResult {
        let pipe = self.pipe.as_mut().unwrap();
        if pipe.status != Status::Success {
            return TickResult::Error;
        }
        match self.phase {
            TransferPhase::Setup => {
                match self.ty {
                    TransferType::ControlIn => {
                        self.phase = TransferPhase::Data;
                        pipe.req = Request::In;
                    }
                    _ => {
                        self.phase = TransferPhase::Status;
                        pipe.len = 0;
                        pipe.req = Request::In;
                    }
                }
                TickResult::Send
            }
            TransferPhase::Data => match self.ty {
                TransferType::PollInterrupt => {
                    let len = pipe.len as usize;
                    self.buf[..len].copy_from_slice(&pipe.buf[..len]);
                    self.actual_len = len as u16;
                    TickResult::Done
                }
                _ => {
                    let len = pipe.len.min(self.target_len - self.actual_len) as usize;
                    self.buf[self.actual_len as usize..][..len].copy_from_slice(&pipe.buf[0..len]);
                    self.actual_len += len as u16;
                    if self.actual_len == self.target_len || (len as u16) < self.packet_size {
                        trace!("IN {:02x?}", &self.buf[0..self.actual_len as usize]);
                        self.phase = TransferPhase::Status;
                        pipe.len = 0;
                        pipe.toggle = 1;
                        pipe.req = Request::Out;
                    }
                    TickResult::Send
                }
            },
            TransferPhase::Status | TransferPhase::Delay | TransferPhase::Idle => {
                self.phase = TransferPhase::Idle;
                TickResult::Done
            }
        }
    }

    fn setup(&mut self, setup: SetupPacket) {
        if let Some(pipe) = &mut self.pipe {
            let len = setup.length();
            pipe.setup(setup);
            pipe.timer = NAK_FRAMES;
            self.phase = TransferPhase::Setup;
            self.ty = if len == 0 {
                TransferType::ControlNone
            } else {
                TransferType::ControlIn
            };
            self.actual_len = 0;
            self.target_len = len;
        }
    }

    fn delay(&mut self, delay_ms: u16) {
        if let Some(pipe) = &mut self.pipe {
            pipe.timer = delay_ms;
            pipe.req = Request::Delay;
            self.phase = TransferPhase::Delay;
        }
    }

    /// Point the pipe at a device's control endpoint.
    fn target(&mut self, addr: u8, low_speed: bool) {
        if let Some(pipe) = &mut self.pipe {
            pipe.addr = addr;
            pipe.low_speed = low_speed;
        }
    }

    /// Poll an interrupt endpoint from here on, which can NAK forever.
    fn poll_interrupt(&mut self, ep: u8, target_len: u16) {
        if let Some(pipe) = &mut self.pipe {
            pipe.ep = ep;
            pipe.toggle = 0;
            pipe.timer = 0;
            pipe.req = Request::In;
        }
        self.ty = TransferType::PollInterrupt;
        self.phase = TransferPhase::Data;
        self.target_len = target_len;
    }
}

fn get_port_status(port: u16) -> SetupPacket {
    SetupPacket::new(
        DEVICE_TO_HOST | CLASS_REQUEST | RECIPIENT_OTHER,
        GET_STATUS,
        0,
        port,
        4,
    )
}

fn set_port_feature(feature: u16, port: u16) -> SetupPacket {
    SetupPacket::new(
        HOST_TO_DEVICE | CLASS_REQUEST | RECIPIENT_OTHER,
        SET_FEATURE,
        feature,
        port,
        0,
    )
}

fn clear_port_feature(feature: u16, port: u16) -> SetupPacket {
    SetupPacket::new(
        HOST_TO_DEVICE | CLASS_REQUEST | RECIPIENT_OTHER,
        CLEAR_FEATURE,
        feature,
        port,
        0,
    )
}

impl UsbTask {
    // Safety: only call this once.
    pub unsafe fn new() -> Self {
        let mut task = Self {
            iface: UserIface::new(),
            transfers: [const { Transfer::new() }; N_PIPES],
            hub: HubState::Recovery,
            n_ports: 0,
            dirty: 0,
            ctrl: CtrlState::Idle,
            port: 0,
            port_status: 0,
            reset_polls: 0,
            ports: [Port::new(); MAX_PORTS + 1],
            restart: None,
            bus_restarts: 0,
            enum_failures: 0,
            soak: Soak::new(),
        };
        task.begin();
        task
    }

    /// Start from a bus that has just been reset.
    fn begin(&mut self) {
        for transfer in &mut self.transfers {
            transfer.pipe = None;
            transfer.phase = TransferPhase::Idle;
        }
        self.hub = HubState::Recovery;
        self.dirty = 0;
        self.ctrl = CtrlState::Idle;
        self.ports = [Port::new(); MAX_PORTS + 1];
        self.restart = None;
        self.transfers[CTRL_PIPE].pipe = Some(self.iface.pipe(CTRL_PIPE));
        let transfer = &mut self.transfers[HUB_PIPE];
        transfer.pipe = Some(self.iface.pipe(HUB_PIPE));
        transfer.target(0, false);
        transfer.packet_size = 64;
        transfer.delay(RESET_RECOVERY);
        self.send(HUB_PIPE);
    }

    /// Poll the USB bus, advancing the state machine.
    ///
    /// This function should be called periodically. There is currently no waker
    /// mechanism in place. Generally the timing is fairly relaxed, but excessive
    /// delays may cause failures (for example, 50ms for set address commands).
    ///
    /// It will probably evolve to return a result, basically a stream of events.
    pub fn poll(&mut self) {
        let mut bits = self.iface.poll();
        while bits != 0 && self.restart.is_none() {
            let ix = bits.trailing_zeros() as usize;
            let pipe = self.iface.pipe(ix);
            if pipe.status != Status::Success {
                console!("pipe {ix} status {:?}", pipe.status);
            }
            self.transfers[ix].pipe = Some(pipe);
            let res = self.transfers[ix].tick();
            match ix {
                HUB_PIPE => self.hub_tick(res),
                CTRL_PIPE => self.ctrl_tick(res),
                _ => self.device_tick(ix, res),
            }
            bits &= bits - 1;
        }
        self.ctrl_next();
        if SOAK {
            self.soak_tick();
        }
        if let Some(kind) = self.restart {
            unsafe { restart(&mut self.iface, kind == Restart::Power) };
            self.begin();
        }
    }

    fn send(&mut self, ix: usize) {
        if let Some(pipe) = self.transfers[ix].pipe.take() {
            self.iface.send_req(pipe);
        }
    }

    fn hub_tick(&mut self, res: TickResult) {
        match res {
            TickResult::Send => return self.send(HUB_PIPE),
            TickResult::Error => {
                console!("hub failed in {:?}", self.hub);
                self.bus_restarts += 1;
                self.restart = Some(Restart::Bus);
                return;
            }
            TickResult::Done => (),
        }
        let transfer = &mut self.transfers[HUB_PIPE];
        match self.hub {
            HubState::Recovery => {
                let setup = SetupPacket::new(
                    DEVICE_TO_HOST,
                    GET_DESCRIPTOR,
                    (DEVICE_DESCRIPTOR as u16) << 8,
                    0,
                    0x12,
                );
                transfer.setup(setup);
                self.hub = HubState::Desc;
            }
            HubState::Desc => {
                let setup = SetupPacket::new(HOST_TO_DEVICE, SET_ADDRESS, HUB_ADDR as u16, 0, 0);
                transfer.setup(setup);
                self.hub = HubState::Address;
            }
            HubState::Address => {
                transfer.delay(ADDRESS_RECOVERY);
                self.hub = HubState::AddressDelay;
            }
            HubState::AddressDelay => {
                transfer.target(HUB_ADDR, false);
                let setup = SetupPacket::new(HOST_TO_DEVICE, SET_CONFIGURATION, 1, 0, 0);
                transfer.setup(setup);
                self.hub = HubState::Configured;
            }
            HubState::Configured => {
                let setup = SetupPacket::new(
                    DEVICE_TO_HOST | CLASS_REQUEST,
                    GET_DESCRIPTOR,
                    (HUB_DESCRIPTOR as u16) << 8,
                    0,
                    9,
                );
                transfer.setup(setup);
                self.hub = HubState::HubDesc;
            }
            HubState::HubDesc => {
                trace!("hub desc {:02x?}", &transfer.buf[0..9]);
                self.n_ports = transfer.buf[2].min(MAX_PORTS as u8);
                transfer.setup(set_port_feature(PORT_POWER, 1));
                self.hub = HubState::PowerPort(1);
            }
            HubState::PowerPort(port) if port < self.n_ports => {
                transfer.setup(set_port_feature(PORT_POWER, port as u16 + 1));
                self.hub = HubState::PowerPort(port + 1);
            }
            HubState::PowerPort(_) => {
                // bPwrOn2PwrGood, in units of 2 ms
                transfer.delay(transfer.buf[5] as u16 * 2 + 1);
                self.hub = HubState::PowerGood;
            }
            HubState::PowerGood => {
                transfer.poll_interrupt(1, 1);
                self.hub = HubState::Polling;
            }
            HubState::Polling => {
                // Bit 0 is the hub itself, and it has nothing we'd act on.
                self.dirty |= transfer.buf[0] as u16 & !1;
            }
        }
        self.send(HUB_PIPE);
    }

    /// Start on a port that needs looking at, if pipe 1 is free.
    fn ctrl_next(&mut self) {
        if self.ctrl == CtrlState::Idle && self.dirty != 0 && self.restart.is_none() {
            self.port = self.dirty.trailing_zeros() as u16;
            self.dirty &= self.dirty - 1;
            self.ctrl_hub(get_port_status(self.port), CtrlState::PortStatus);
        }
    }

    /// Run a control transfer to the hub on pipe 1.
    fn ctrl_hub(&mut self, setup: SetupPacket, state: CtrlState) {
        let transfer = &mut self.transfers[CTRL_PIPE];
        transfer.target(HUB_ADDR, false);
        transfer.packet_size = 64;
        transfer.setup(setup);
        self.ctrl = state;
        self.send(CTRL_PIPE);
    }

    /// Run a control transfer to the device being enumerated on pipe 1.
    fn ctrl_device(&mut self, addr: u8, setup: SetupPacket, state: CtrlState) {
        let low_speed = self.ports[self.port as usize].low_speed;
        let transfer = &mut self.transfers[CTRL_PIPE];
        transfer.target(addr, low_speed);
        transfer.setup(setup);
        self.ctrl = state;
        self.send(CTRL_PIPE);
    }

    fn ctrl_delay(&mut self, delay_ms: u16, state: CtrlState) {
        self.transfers[CTRL_PIPE].delay(delay_ms);
        self.ctrl = state;
        self.send(CTRL_PIPE);
    }

    fn reset_port(&mut self) {
        self.reset_polls = 0;
        self.ports[self.port as usize].phase = PortPhase::Enumerating;
        self.ctrl_hub(set_port_feature(PORT_RESET, self.port), CtrlState::DidReset);
    }

    fn enum_failed(&mut self) {
        self.enum_failures += 1;
        let port = &mut self.ports[self.port as usize];
        port.attempts += 1;
        console!(
            "enumeration failed in {:?} {:?}, attempt {}",
            self.ctrl,
            self.transfers[CTRL_PIPE].phase,
            port.attempts
        );
        if port.attempts < ENUM_ATTEMPTS {
            self.reset_port();
        } else {
            port.phase = PortPhase::Failed;
            self.ctrl = CtrlState::Idle;
        }
    }

    fn ctrl_tick(&mut self, res: TickResult) {
        match res {
            TickResult::Send => return self.send(CTRL_PIPE),
            TickResult::Error => {
                match self.ctrl {
                    CtrlState::GotDesc
                    | CtrlState::Address
                    | CtrlState::GotConfig
                    | CtrlState::Configured
                    | CtrlState::GotReportDesc
                    | CtrlState::ProtocolSet => self.enum_failed(),
                    _ => {
                        console!("hub failed in {:?}", self.ctrl);
                        self.bus_restarts += 1;
                        self.restart = Some(Restart::Bus);
                    }
                }
                return;
            }
            TickResult::Done => (),
        }
        match self.ctrl {
            CtrlState::Idle => (),
            CtrlState::PortStatus => self.got_port_status(),
            CtrlState::ClearedChange => {
                self.ctrl_hub(get_port_status(self.port), CtrlState::PortStatus);
            }
            CtrlState::Debounce => self.reset_port(),
            CtrlState::DidReset => self.ctrl_delay(RESET_WAIT, CtrlState::ResetWait),
            CtrlState::ResetWait => {
                self.ctrl_hub(get_port_status(self.port), CtrlState::PortStatus);
            }
            CtrlState::ResetRecovery => {
                // Device has been reset, is now in default state, with address 0 (see Figure 9-1)
                let setup = SetupPacket::new(
                    DEVICE_TO_HOST,
                    GET_DESCRIPTOR,
                    (DEVICE_DESCRIPTOR as u16) << 8,
                    0,
                    0x8,
                );
                self.transfers[CTRL_PIPE].packet_size = 8;
                self.ctrl_device(0, setup, CtrlState::GotDesc);
            }
            CtrlState::GotDesc => {
                let transfer = &mut self.transfers[CTRL_PIPE];
                trace!("device desc {:02x?}", &transfer.buf[0..8]);
                if transfer.actual_len != 8 || transfer.buf[1] != DEVICE_DESCRIPTOR {
                    return self.enum_failed();
                }
                transfer.packet_size = transfer.buf[7] as u16;
                // probably should get full descriptor here, but we cut corners
                let setup = SetupPacket::new(HOST_TO_DEVICE, SET_ADDRESS, self.port + 1, 0, 0);
                self.ctrl_device(0, setup, CtrlState::Address);
            }
            CtrlState::Address => self.ctrl_delay(ADDRESS_RECOVERY, CtrlState::AddressDelay),
            CtrlState::AddressDelay => {
                // As much of it as fits, the start being what matters.
                let setup = SetupPacket::new(
                    DEVICE_TO_HOST,
                    GET_DESCRIPTOR,
                    (CONFIG_DESCRIPTOR as u16) << 8,
                    0,
                    64,
                );
                self.ctrl_device(self.port as u8 + 1, setup, CtrlState::GotConfig);
            }
            CtrlState::GotConfig => {
                let transfer = &self.transfers[CTRL_PIPE];
                let config = &transfer.buf[..transfer.actual_len as usize];
                trace!("config desc {config:02x?}");
                if config.len() < 9 || config[1] != CONFIG_DESCRIPTOR {
                    return self.enum_failed();
                }
                let interface = Interface::parse(config);
                let value = config[5] as u16;
                trace!("port {} using {interface:?}", self.port);
                self.ports[self.port as usize].interface = interface;
                let setup = SetupPacket::new(HOST_TO_DEVICE, SET_CONFIGURATION, value, 0, 0);
                self.ctrl_device(self.port as u8 + 1, setup, CtrlState::Configured);
            }
            CtrlState::Configured => {
                let interface = self.ports[self.port as usize].interface;
                if interface.kind == Kind::Mouse {
                    // Devices start out in report protocol, which is what
                    // this describes. Boot protocol isn't relied on, as
                    // there are mice that accept it and carry on as before.
                    let setup = SetupPacket::new(
                        DEVICE_TO_HOST | RECIPIENT_INTERFACE,
                        GET_DESCRIPTOR,
                        (REPORT_DESCRIPTOR as u16) << 8,
                        interface.number as u16,
                        interface.report_len.min(TRANSFER_BUF as u16),
                    );
                    self.ctrl_device(self.port as u8 + 1, setup, CtrlState::GotReportDesc);
                } else {
                    self.ctrl = CtrlState::Idle;
                    self.start_device();
                }
            }
            CtrlState::GotReportDesc => {
                let transfer = &self.transfers[CTRL_PIPE];
                let desc = &transfer.buf[..transfer.actual_len as usize];
                trace!("report desc {desc:02x?}");
                let layout = mouse::Layout::parse(desc);
                trace!("mouse layout {layout:?}");
                let port = &mut self.ports[self.port as usize];
                if let Some(layout) = layout {
                    port.layout = layout;
                    self.ctrl = CtrlState::Idle;
                    self.start_device();
                } else {
                    port.layout = mouse::Layout::BOOT;
                    let setup = SetupPacket::new(
                        HOST_TO_DEVICE | CLASS_REQUEST | RECIPIENT_INTERFACE,
                        SET_PROTOCOL,
                        0,
                        port.interface.number as u16,
                        0,
                    );
                    self.ctrl_device(self.port as u8 + 1, setup, CtrlState::ProtocolSet);
                }
            }
            CtrlState::ProtocolSet => {
                self.ctrl = CtrlState::Idle;
                self.start_device();
            }
        }
    }

    fn got_port_status(&mut self) {
        let transfer = &self.transfers[CTRL_PIPE];
        let status = u16::from_le_bytes(transfer.buf[0..2].try_into().unwrap());
        let changes = u16::from_le_bytes(transfer.buf[2..4].try_into().unwrap());
        trace!("port {} status {status:04x} changes {changes:04x}", self.port);
        self.port_status = status;
        let port = &mut self.ports[self.port as usize];
        let changes = changes & PORT_CHANGES;
        if changes != 0 {
            let change = changes.trailing_zeros() as u16;
            if change == 0 {
                port.phase = PortPhase::Empty;
            }
            let setup = clear_port_feature(change + 16, self.port);
            return self.ctrl_hub(setup, CtrlState::ClearedChange);
        }
        if status & PORT_CONNECTED == 0 {
            port.phase = PortPhase::Empty;
            self.ctrl = CtrlState::Idle;
            return;
        }
        match port.phase {
            PortPhase::Empty => {
                port.attempts = 0;
                self.ctrl_delay(CONNECT_DEBOUNCE, CtrlState::Debounce);
            }
            PortPhase::Enumerating if status & PORT_RESETTING != 0 => {
                self.reset_polls += 1;
                if self.reset_polls < RESET_POLLS {
                    self.ctrl_delay(RESET_WAIT, CtrlState::ResetWait);
                } else {
                    self.enum_failed();
                }
            }
            PortPhase::Enumerating if status & PORT_ENABLED != 0 => {
                port.low_speed = status & PORT_LOW_SPEED != 0;
                self.ctrl_delay(RESET_RECOVERY, CtrlState::ResetRecovery);
            }
            PortPhase::Enumerating => self.enum_failed(),
            PortPhase::Running if status & PORT_ENABLED == 0 => {
                // Its polling will fail, if it hasn't already.
                port.phase = PortPhase::Empty;
                self.ctrl_delay(CONNECT_DEBOUNCE, CtrlState::Debounce);
            }
            PortPhase::Running | PortPhase::Failed => self.ctrl = CtrlState::Idle,
        }
    }

    /// Start polling the device just enumerated, each port having its own
    /// pipe and address.
    fn start_device(&mut self) {
        let port = self.port as usize;
        self.ports[port].phase = PortPhase::Running;
        let ix = port + 1;
        let mut pipe = self.iface.pipe(ix);
        pipe.addr = ix as u8;
        pipe.low_speed = self.ports[port].low_speed;
        let transfer = &mut self.transfers[ix];
        transfer.pipe = Some(pipe);
        transfer.poll_interrupt(self.ports[port].interface.ep, 8);
        self.send(ix);
    }

    fn device_tick(&mut self, ix: usize, res: TickResult) {
        let port = ix - 1;
        match res {
            TickResult::Done => {
                let transfer = &self.transfers[ix];
                let report = &transfer.buf[0..transfer.actual_len as usize];
                if self.ports[port].interface.kind == Kind::Mouse {
                    mouse::report(report, &self.ports[port].layout);
                } else {
                    trace!("report {report:02x?}");
                }
                if self.ports[port].phase == PortPhase::Running {
                    self.soak.reported = true;
                }
                self.send(ix);
            }
            TickResult::Send => self.send(ix),
            TickResult::Error => {
                // Look at the port again; the hub knows if it was unplugged.
                if let Some(pipe) = self.transfers[ix].pipe.take() {
                    self.iface.release_pipe(pipe);
                }
                if self.ports[port].phase == PortPhase::Running {
                    self.ports[port].phase = PortPhase::Empty;
                    self.dirty |= 1 << port;
                }
            }
        }
    }

    fn soak_tick(&mut self) {
        let now = super::now_us();
        let mode = self.soak.mode;
        if let Some(reported) = self.soak.settling {
            if now.wrapping_sub(reported) >= SOAK_SETTLE_US {
                self.soak_next(now);
            }
        } else if self.soak.reported {
            let ms = now.wrapping_sub(self.soak.start) / 1000;
            self.soak.ok[mode] += 1;
            self.soak.max_ms[mode] = self.soak.max_ms[mode].max(ms);
            self.soak.settling = Some(now);
        } else if now.wrapping_sub(self.soak.start) >= SOAK_DEADLINE_US {
            self.soak.failed[mode] += 1;
            console!(
                "soak: {} failed, hub {:?} ctrl {:?} port {} status {:04x} phase {:?}",
                MODE_NAMES[mode],
                self.hub,
                self.ctrl,
                self.port,
                self.port_status,
                self.ports[self.port as usize].phase,
            );
            self.soak_next(now);
        }
    }

    fn soak_next(&mut self, now: u32) {
        self.soak.mode = (self.soak.mode + 1) % N_MODES;
        self.soak.start = now;
        self.soak.settling = None;
        self.soak.reported = false;
        match self.soak.mode {
            0 => {
                for port in 1..=self.n_ports as usize {
                    if self.ports[port].phase == PortPhase::Running {
                        self.ports[port].phase = PortPhase::Empty;
                        self.dirty |= 1 << port;
                    }
                }
            }
            1 => self.restart = Some(Restart::Bus),
            _ => self.restart = Some(Restart::Power),
        }
    }

    /// Print counts of what's gone wrong, and the soak test's outcomes.
    pub fn report(&self) {
        let [wrong, crc, no_reply] = self.iface.errors();
        console!(
            "usb errors: wrong {wrong} crc {crc} no reply {no_reply}, enum failures {}, bus restarts {}",
            self.enum_failures,
            self.bus_restarts,
        );
        if SOAK {
            for mode in 0..N_MODES {
                console!(
                    "soak {}: ok {} failed {} max {} ms",
                    MODE_NAMES[mode],
                    self.soak.ok[mode],
                    self.soak.failed[mode],
                    self.soak.max_ms[mode],
                );
            }
        }
    }
}

impl Port {
    const fn new() -> Self {
        Self {
            phase: PortPhase::Empty,
            attempts: 0,
            low_speed: false,
            interface: Interface::new(),
            layout: mouse::Layout::BOOT,
        }
    }
}

impl Soak {
    const fn new() -> Self {
        Self {
            // The first enumeration is after a bus reset.
            mode: 1,
            start: 0,
            settling: None,
            reported: false,
            ok: [0; N_MODES],
            failed: [0; N_MODES],
            max_ms: [0; N_MODES],
        }
    }
}
