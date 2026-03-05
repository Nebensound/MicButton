//! ATtiny45 Mic Button Controller
//!
//! Pin assignment:
//!   PB0 = Button 1 + Mic Click (shared, alternates In/Out)
//!   PB1 = Status LED (output)
//!   PB2 = Button 2 (input only)
//!   PB3 = Mic Status (input, HIGH = mic on)
//!
//! Behavior (both buttons identical):
//!   - Short press: Mic toggle (on → 10 s timer, off → immediately off)
//!   - Press and hold: Mic on while held, off on release
//!   - Mic is controlled via short GPIO pulse (click) on PB0
//!   - Mic status is read via PB3; corrected after >500 ms mismatch
//!   - Non-blocking via Timer0 interrupt + state machine

#![no_std]
#![no_main]
#![feature(abi_avr_interrupt)]

use avr_device::attiny85::Peripherals;
use avr_device::interrupt;
use core::cell::Cell;
use panic_halt as _;

use mic_button::logic::{self, Action, ButtonInput, Controller, LedState};

// ── Global state (interrupt-safe via Mutex<Cell>) ──

/// Millisecond counter, incremented by the timer interrupt
static MILLIS: interrupt::Mutex<Cell<u32>> = interrupt::Mutex::new(Cell::new(0));

// Timer0 Compare Match A – fires every ~1 ms
#[avr_device::interrupt(attiny85)]
fn TIMER0_COMPA() {
    interrupt::free(|cs| {
        let m = MILLIS.borrow(cs);
        m.set(m.get().wrapping_add(1));
    });
}

/// Read current milliseconds
fn millis() -> u32 {
    interrupt::free(|cs| MILLIS.borrow(cs).get())
}

/// Mic click on PB0: briefly switch pin to output, send pulse, switch back to input
fn mic_click(dp: &Peripherals) {
    dp.PORTB.portb().modify(|_, w| w.pb0().clear_bit());
    dp.PORTB.ddrb().modify(|_, w| w.pb0().set_bit());

    let start = millis();
    while millis().wrapping_sub(start) < logic::CLICK_MS {}

    dp.PORTB.ddrb().modify(|_, w| w.pb0().clear_bit());
    dp.PORTB.portb().modify(|_, w| w.pb0().set_bit());
}

/// Turn status LED on/off (PB1)
fn led_set(dp: &Peripherals, on: bool) {
    if on {
        dp.PORTB.portb().modify(|_, w| w.pb1().set_bit());
    } else {
        dp.PORTB.portb().modify(|_, w| w.pb1().clear_bit());
    }
}

/// Startup blink: flash 2× briefly to indicate firmware is running
fn startup_blink(dp: &Peripherals) {
    for _ in 0..2 {
        led_set(dp, true);
        let start = millis();
        while millis().wrapping_sub(start) < logic::STARTUP_BLINK_MS {}
        led_set(dp, false);
        let start = millis();
        while millis().wrapping_sub(start) < logic::STARTUP_BLINK_MS {}
    }
}

/// Apply state machine actions to hardware
fn apply_actions(dp: &Peripherals, actions: &[Action]) {
    for action in actions {
        match action {
            Action::MicClick => mic_click(dp),
            Action::Led(led_state) => match led_state {
                LedState::Off => led_set(dp, false),
                LedState::On => led_set(dp, true),
                LedState::Blink(on) => led_set(dp, *on),
            },
        }
    }
}

#[avr_device::entry]
fn main() -> ! {
    let dp = Peripherals::take().unwrap();

    // ── Configure GPIO ──
    dp.PORTB.ddrb().write(|w| {
        w.pb0()
            .clear_bit() // Input (Button 1 / Mic Click shared)
            .pb1()
            .set_bit() // Output (Status LED)
            .pb2()
            .clear_bit() // Input (Button 2)
            .pb3()
            .clear_bit() // Input (Mic Status)
    });
    dp.PORTB.portb().write(|w| {
        w.pb2()
            .set_bit() // Pull-up Button 2 (PB0 has external pull-up)
    });

    // ── Timer0: CTC mode, ~1 ms interrupt at 8 MHz ──
    dp.TC0.tccr0a().write(|w| w.wgm0().ctc());
    dp.TC0.tccr0b().write(|w| w.cs0().prescale_64());
    dp.TC0.ocr0a().write(|w| unsafe { w.bits(124) }); // 8MHz / 64 / 125 = 1000 Hz
    dp.TC0.timsk().write(|w| w.ocie0a().set_bit());

    // SAFETY: All shared data (MILLIS) is protected via interrupt::Mutex<Cell>
    // and only accessed within interrupt::free().
    unsafe { interrupt::enable() };

    // ── Startup blink: 2× flash → "firmware running" ──
    startup_blink(&dp);

    let mut ctrl = Controller::new();

    loop {
        let input = ButtonInput {
            now: millis(),
            btn1: dp.PORTB.pinb().read().pb0().bit_is_clear(),
            btn2: dp.PORTB.pinb().read().pb2().bit_is_clear(),
            mic_on: dp.PORTB.pinb().read().pb3().bit_is_set(),
        };

        let actions = ctrl.update(&input);
        apply_actions(&dp, &actions);
    }
}
