mod host_task;
mod iface;
mod timing;
mod wire;

use core::{cell::UnsafeCell, mem::MaybeUninit};

use cortex_m::peripheral::NVIC;
use hal::{
    pac::{Interrupt, PIO0, RESETS},
    pio::{Buffers, PIOExt, Rx, StateMachine, Stopped, Tx, ValidStateMachine, SM0, SM1},
};
use pio::Instruction;
use rp235x_hal as hal;

use self::wire::PID_SOF;

pub use self::host_task::UsbTask;
pub use self::timing::report as timing_report;

struct UsbPioWrapper<P: PIOExt>(UnsafeCell<MaybeUninit<UsbPio<P>>>);

unsafe impl<P: PIOExt> Sync for UsbPioWrapper<P> {}

static USB_PIO: UsbPioWrapper<PIO0> = UsbPioWrapper(UnsafeCell::new(MaybeUninit::uninit()));

const DP_PIN: u8 = 1;
const DM_PIN: u8 = 2;
const USB_5V_PIN: usize = 11;

const IO_BANK0: usize = 0x40028000;
const PADS_BANK0: usize = 0x40038000;
const SIO_GPIO_OUT_SET: usize = 0xd0000018;
const SIO_GPIO_OE_SET: usize = 0xd0000038;
const TIMER0_INTR: usize = 0x400b003c;
const TIMER0_INTE: usize = 0x400b0040;

/// Clear isolation; input enable, 4 mA, pull-down, Schmitt.
const PAD_CONFIG: u32 = 0x56;
const FUNCSEL_SIO: u32 = 5;
const FUNCSEL_PIO0: u32 = 6;
/// GPIO_CTRL's INOVER field set to invert.
const INOVER_INVERT: u32 = 1 << 16;

// Very low level PIO functions

#[inline]
#[link_section = ".data"]
fn pio_addr<S: ValidStateMachine, State>(_sm: &StateMachine<S, State>) -> *mut u32 {
    (0x50200000 + S::PIO::id() * 0x100000) as *mut u32
}

#[inline]
#[link_section = ".data"]
unsafe fn write_bitmask_clear(register: *mut u32, bits: u32) {
    let alias = (register as usize + 0x3000) as *mut u32;
    core::ptr::write_volatile(alias, bits);
}

#[inline]
#[link_section = ".data"]
unsafe fn pio_sm_stop<S: ValidStateMachine, State>(sm: &StateMachine<S, State>) {
    write_bitmask_clear(pio_addr(sm), 1 << S::id());
}

#[inline]
#[link_section = ".data"]
fn fast_get_timer() -> u32 {
    let timer_addr = 0x400b0028;
    unsafe { core::ptr::read_volatile(timer_addr as *const u32) }
}

#[inline]
#[link_section = ".data"]
/// Schedules the alarm at timestamp.
///
/// Doesn't try to do anything fancy about detecting timestamps in the past.
unsafe fn fast_alarm_schedule(timestamp: u32) {
    let timer_addr = 0x400b0010;
    unsafe {
        core::ptr::write_volatile(timer_addr as *mut u32, timestamp);
    }
}

/// Must match usb_tx.sy's Stamps.
#[repr(C)]
struct Stamps {
    tx_start: u32,
    tx_idle: u32,
    rx_start: u32,
    rx_eop: u32,
    rx_max_poll: u32,
}

#[no_mangle]
static mut USB_STAMPS: Stamps = Stamps {
    tx_start: 0,
    tx_idle: 0,
    rx_start: 0,
    rx_eop: 0,
    rx_max_poll: 0,
};

extern "C" {
    fn usb_tx_token(pid: u32, data: u32, low_speed: u32);
    fn usb_handle_requests();
}

#[no_mangle]
#[link_section = ".data"]
unsafe extern "C" fn usb_record_rx(low_speed: u32) {
    timing::record(
        timing::TX_TO_RX + low_speed as usize,
        USB_STAMPS.rx_start.wrapping_sub(USB_STAMPS.tx_idle),
    );
    timing::record(timing::RX_POLL + low_speed as usize, USB_STAMPS.rx_max_poll);
}

#[no_mangle]
#[link_section = ".data"]
unsafe extern "C" fn usb_record_ack(low_speed: u32) {
    timing::record(
        timing::EOP_TO_ACK + low_speed as usize,
        USB_STAMPS.tx_start.wrapping_sub(USB_STAMPS.rx_eop),
    );
}

#[no_mangle]
#[link_section = ".data"]
unsafe extern "C" fn usb_record_data(token_end: u32, low_speed: u32) {
    timing::record(
        timing::TOKEN_TO_DATA + low_speed as usize,
        USB_STAMPS.tx_start.wrapping_sub(token_end),
    );
}

pub fn now_us() -> u32 {
    fast_get_timer()
}

// Clock dividers for clk_sys at 144 MHz. TX runs at 4 cycles per bit and RX
// at 12.
const TX_DIV_FULL_SPEED: u16 = 3;
const RX_DIV_FULL_SPEED: u16 = 1;

struct UsbPio<PIO: PIOExt> {
    tx_sm: StateMachine<(PIO, SM0), Stopped>,
    // The interrupt side drives these through their registers; they're here
    // to stay owned.
    #[expect(dead_code)]
    tx: Tx<(PIO, SM0)>,
    #[expect(dead_code)]
    rx_sm: StateMachine<(PIO, SM1), Stopped>,
    #[expect(dead_code)]
    rx: Rx<(PIO, SM1)>,
    #[expect(dead_code)]
    pio: rp235x_hal::pio::PIO<PIO>,
    state: u32,
    frame_number: u32,
    /// Timestamp (in us) of last SOF packet
    sof_timestamp: u32,
}

impl<PIO: PIOExt> UsbPio<PIO> {
    // TODO: make more pin-agile
    fn new(pio: PIO, resets: &mut RESETS) -> Self {
        unsafe {
            configure_pin(DP_PIN as usize, FUNCSEL_PIO0 | INOVER_INVERT);
            configure_pin(DM_PIN as usize, FUNCSEL_PIO0 | INOVER_INVERT);
        }
        let (mut pio, sm0, sm1, _, _) = pio.split(resets);
        let usb_tx_program = pio_proc::pio_file!("src/pio_usb/usb_tx.pio");
        let tx_installed = pio.install(&usb_tx_program.program).unwrap();
        let (tx_sm, _, tx) = rp235x_hal::pio::PIOBuilder::from_installed_program(tx_installed)
            .set_pins(DP_PIN, 2)
            .out_pins(DP_PIN, 2)
            .in_pin_base(DP_PIN)
            .clock_divisor_fixed_point(TX_DIV_FULL_SPEED, 0)
            .pull_threshold(8)
            .autopull(true)
            .buffers(Buffers::OnlyTx)
            .build(sm0);
        let rx_program = pio_proc::pio_file!("src/pio_usb/usb_rx.pio");
        let rx_installed = pio.install(&rx_program.program).unwrap();
        let (rx_sm, rx, _) = rp235x_hal::pio::PIOBuilder::from_installed_program(rx_installed)
            .in_pin_base(DP_PIN)
            .jmp_pin(DM_PIN)
            .clock_divisor_fixed_point(RX_DIV_FULL_SPEED, 0)
            .push_threshold(8)
            .autopush(true)
            .buffers(Buffers::OnlyRx)
            .build(sm1);
        // SM1_SHIFTCTRL's IN_COUNT, which this HAL doesn't expose: mov x, pins
        // has to see only D+.
        unsafe {
            let shiftctrl = (0x50200000 + PIO::id() * 0x100000 + 0xe8) as *mut u32;
            shiftctrl.write_volatile((shiftctrl.read_volatile() & !0x1f) | 1);
        }
        Self {
            tx_sm,
            tx,
            rx_sm,
            rx,
            pio,
            state: 0,
            frame_number: 0,
            sof_timestamp: 0,
        }
    }

    #[link_section = ".data"]
    fn bus_reset(&mut self, reset: bool) {
        unsafe {
            pio_sm_stop(&self.tx_sm);
        }
        if reset {
            self.tx_sm.exec_instruction(Instruction {
                operands: pio::InstructionOperands::SET {
                    destination: pio::SetDestination::PINS,
                    data: 0,
                },
                delay: 0,
                side_set: None,
            });
        }
        self.tx_sm.exec_instruction(Instruction {
            operands: pio::InstructionOperands::SET {
                destination: pio::SetDestination::PINDIRS,
                data: if reset { 3 } else { 0 },
            },
            delay: 0,
            side_set: None,
        });
    }

    #[link_section = ".data"]
    fn wait_next_sof(&mut self) {
        self.sof_timestamp = self.sof_timestamp.wrapping_add(1000);
        self.frame_number = self.frame_number.wrapping_add(1);
        unsafe {
            fast_alarm_schedule(self.sof_timestamp);
        }
        self.state = 2;
    }

}

unsafe fn configure_pin(pin: usize, ctrl: u32) {
    ((PADS_BANK0 + 4 + 4 * pin) as *mut u32).write_volatile(PAD_CONFIG);
    ((IO_BANK0 + 4 + 8 * pin) as *mut u32).write_volatile(ctrl);
}

/// Set up the state machines and power the host port. Call before core 1
/// starts, as it touches RESETS.
pub unsafe fn init(pio: PIO0, resets: &mut RESETS) {
    configure_pin(USB_5V_PIN, FUNCSEL_SIO);
    (SIO_GPIO_OUT_SET as *mut u32).write_volatile(1 << USB_5V_PIN);
    (SIO_GPIO_OE_SET as *mut u32).write_volatile(1 << USB_5V_PIN);
    (*USB_PIO.0.get()).write(UsbPio::new(pio, resets));
}

/// Start the host timer interrupt on the calling core, at the highest
/// priority, as it blocks through timing-critical turnarounds.
pub unsafe fn start() {
    timing::init();
    fast_alarm_schedule(fast_get_timer().wrapping_add(12_000));
    (TIMER0_INTE as *mut u32).write_volatile(1);
    cortex_m::Peripherals::steal()
        .NVIC
        .set_priority(Interrupt::TIMER0_IRQ_0, 0);
    NVIC::unmask(Interrupt::TIMER0_IRQ_0);
}

#[link_section = ".data"]
#[no_mangle]
pub unsafe extern "C" fn TIMER0_IRQ_0() {
    let usb_pio = (*USB_PIO.0.get()).assume_init_mut();
    (TIMER0_INTR as *mut u32).write_volatile(1);
    match usb_pio.state {
        0 => {
            usb_pio.bus_reset(true);
            fast_alarm_schedule(fast_get_timer().wrapping_add(12_000));
            usb_pio.state = 1;
        }
        1 => {
            usb_pio.bus_reset(false);
            fast_alarm_schedule(fast_get_timer().wrapping_add(1_000));
            usb_pio.state = 2;
        }
        2 => {
            let entry = timing::cycles();
            if usb_pio.frame_number == 0 {
                usb_pio.sof_timestamp = fast_get_timer();
            }
            timing::record(
                timing::SOF_LATE,
                fast_get_timer().wrapping_sub(usb_pio.sof_timestamp),
            );
            usb_tx_token(PID_SOF as u32, usb_pio.frame_number & 0x7ff, 0);
            usb_handle_requests();
            usb_pio.wait_next_sof();
            timing::record(timing::HANDLER, timing::cycles().wrapping_sub(entry));
        }
        _ => (),
    }
}
