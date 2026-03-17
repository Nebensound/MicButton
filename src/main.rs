#![cfg_attr(target_arch = "avr", no_std)]
#![cfg_attr(target_arch = "avr", no_main)]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

#[cfg(target_arch = "avr")]
mod avr {
    use avr_device::attiny85::Peripherals;
    use avr_device::interrupt;
    use core::cell::Cell;
    use panic_halt as _;

    use mic_button::logic::{self, mic_on_from_adc, Controller, Input};

    static MILLIS: interrupt::Mutex<Cell<u32>> = interrupt::Mutex::new(Cell::new(0));

    /// ADC midpoint threshold between mic-off (≈ 2.7 V → ADC ≈ 552) and
    /// mic-on (≈ 3.7 V → ADC ≈ 757) with VCC = 5 V as reference.
    const ADC_MIC_THRESHOLD: u16 = 655;

    #[avr_device::interrupt(attiny85)]
    fn TIMER0_COMPA() {
        interrupt::free(|cs| {
            let m = MILLIS.borrow(cs);
            m.set(m.get().wrapping_add(1));
        });
    }

    fn millis() -> u32 {
        interrupt::free(|cs| MILLIS.borrow(cs).get())
    }

    /// Trigger mic click via D882 transistor on PB1.
    /// PB1 HIGH -> D882 conducts -> SW1<->SW2 bridged (= button press).
    fn mic_click(dp: &Peripherals) {
        dp.PORTB.portb().modify(|_, w| w.pb1().set_bit());
        let start = millis();
        while millis().wrapping_sub(start) < logic::CLICK_MS {}
        dp.PORTB.portb().modify(|_, w| w.pb1().clear_bit());
    }

    fn led_set(dp: &Peripherals, on: bool) {
        if on {
            dp.PORTB.portb().modify(|_, w| w.pb4().set_bit());
        } else {
            dp.PORTB.portb().modify(|_, w| w.pb4().clear_bit());
        }
    }

    fn delay(ms: u32) {
        let start = millis();
        while millis().wrapping_sub(start) < ms {}
    }

    /// Enable the ADC and configure the clock prescaler.
    /// Must be called once before the first `mic_on_adc` call.
    fn adc_init(dp: &Peripherals) {
        // Disable digital input buffer on PB3/ADC3 to reduce noise.
        // DIDR0 bit 3 = ADC3D.
        dp.ADC.didr0().modify(|r, w| unsafe { w.bits(r.bits() | 0x08) });
        // ADCSRA: ADEN=1, prescaler 8 (ADPS1+ADPS0) → 1 MHz / 8 = 125 kHz ADC clock.
        dp.ADC.adcsra().write(|w| unsafe { w.bits(0x83) });
    }

    /// Sample PB3/ADC3 and return `true` when the voltage exceeds
    /// `ADC_MIC_THRESHOLD` (mic-on ≈ 3.7 V, mic-off ≈ 2.7 V, VCC = 5 V).
    fn mic_on_adc(dp: &Peripherals) -> bool {
        // ADMUX: VCC reference (REFS2:1:0 = 000), right-align (ADLAR=0), ADC3 (MUX = 0b0011).
        dp.ADC.admux().write(|w| unsafe { w.bits(0x03) });
        // Set ADSC to trigger a single conversion.
        dp.ADC
            .adcsra()
            .modify(|r, w| unsafe { w.bits(r.bits() | 0x40) });
        // Poll until ADSC clears (conversion done, ≈ 104 µs at 125 kHz).
        while dp.ADC.adcsra().read().bits() & 0x40 != 0 {}
        // avr-device 0.8 exposes a combined 16-bit ADC register.
        mic_on_from_adc(dp.ADC.adc().read().bits())
    }

    #[avr_device::entry]
    fn main() -> ! {
        let dp = Peripherals::take().unwrap();

        // PB1 = D882 base (output, LOW = off)
        // PB4 = status LED (output)
        // PB2 = button 2 (input, internal pull-up)
        // PB0 = button 1 / SW1 read (input, no pull-up, 3.3V PCB pull-up)
        // PB3 = mic status (input)
        dp.PORTB.ddrb().write(|w| w.pb1().set_bit().pb4().set_bit());
        dp.PORTB.portb().write(|w| w.pb2().set_bit());

        // Timer0 CTC: 1 MHz / 8 / 125 = 1 kHz (1 ms tick)
        dp.TC0.tccr0a().write(|w| w.wgm0().ctc());
        dp.TC0.tccr0b().write(|w| w.cs0().prescale_8());
        dp.TC0.ocr0a().write(|w| unsafe { w.bits(124) });
        dp.TC0.timsk().write(|w| w.ocie0a().set_bit());

        // SAFETY: MILLIS protected via interrupt::Mutex<Cell>
        unsafe { interrupt::enable() };

        adc_init(&dp);

        // Startup blink: 3x flash on PB4
        for _ in 0..3 {
            led_set(&dp, true);
            delay(100);
            led_set(&dp, false);
            delay(100);
        }

        let mut ctrl = Controller::new();

        loop {
            let out = ctrl.update(&Input {
                now: millis(),
                btn1: dp.PORTB.pinb().read().pb0().bit_is_clear(),
                btn2: dp.PORTB.pinb().read().pb2().bit_is_clear(),
                mic_on: mic_on_adc(&dp),
            });

            if out.click {
                mic_click(&dp);
            }
            led_set(&dp, out.led);
        }
    }
}

#[cfg(not(target_arch = "avr"))]
fn main() {}
