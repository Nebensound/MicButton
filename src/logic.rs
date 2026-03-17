pub const HOLD_MS: u32 = 500;
pub const TIMER_MS: u32 = 10_000;
pub const CLICK_MS: u32 = 100;
pub const BLINK_MS: u32 = 500;
pub const DEBOUNCE_MS: u32 = 30;
pub const GAP_MS: u32 = 200;
pub const SYNC_MS: u32 = 500;

/// ADC midpoint between mic-off (≈ 2.7 V → raw ≈ 552) and
/// mic-on (≈ 3.7 V → raw ≈ 757) with VCC = 5 V as reference.
pub const ADC_MIC_THRESHOLD: u16 = 655;

/// Convert a 10-bit ADC reading (VCC reference) to a mic-on boolean.
#[inline]
pub fn mic_on_from_adc(raw: u16) -> bool {
    raw > ADC_MIC_THRESHOLD
}

pub struct Input {
    pub now: u32,
    pub btn1: bool,
    pub btn2: bool,
    pub mic_on: bool,
}

pub struct Output {
    pub click: bool,
    pub led: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Pressing,
    Timed,
    Held,
    /// Button 1 was pressed while in Held – suppress release-click until Button 2
    /// is released, then return to Idle without emitting another click.
    SuppressedUntilRelease,
    Gap,
}

pub struct Controller {
    pub state: State,
    press_start: u32,
    timer_start: u32,
    gap_start: u32,
    was_active: bool,
    mic_on_at_press: bool,
    physical_toggle: bool,
    raw_btn1: bool,
    raw_btn2: bool,
    stable_btn1: bool,
    stable_btn2: bool,
    last_btn1: bool,
    last_btn2: bool,
    btn1_change_at: u32,
    btn2_change_at: u32,
    mismatch_since: u32,
    mismatch_active: bool,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            press_start: 0,
            timer_start: 0,
            gap_start: 0,
            was_active: false,
            mic_on_at_press: false,
            physical_toggle: false,
            raw_btn1: false,
            raw_btn2: false,
            stable_btn1: false,
            stable_btn2: false,
            last_btn1: false,
            last_btn2: false,
            btn1_change_at: 0,
            btn2_change_at: 0,
            mismatch_since: 0,
            mismatch_active: false,
        }
    }

    pub fn update(&mut self, input: &Input) -> Output {
        let now = input.now;
        let mut click = false;

        // Debounce
        if input.btn1 != self.raw_btn1 {
            self.raw_btn1 = input.btn1;
            self.btn1_change_at = now;
        }
        if now.wrapping_sub(self.btn1_change_at) >= DEBOUNCE_MS {
            self.stable_btn1 = self.raw_btn1;
        }
        if input.btn2 != self.raw_btn2 {
            self.raw_btn2 = input.btn2;
            self.btn2_change_at = now;
        }
        if now.wrapping_sub(self.btn2_change_at) >= DEBOUNCE_MS {
            self.stable_btn2 = self.raw_btn2;
        }

        // Edge detection
        let pressed1 = self.stable_btn1 && !self.last_btn1;
        let released1 = !self.stable_btn1 && self.last_btn1;
        let pressed2 = self.stable_btn2 && !self.last_btn2;
        let released2 = !self.stable_btn2 && self.last_btn2;
        self.last_btn1 = self.stable_btn1;
        self.last_btn2 = self.stable_btn2;

        let any_pressed = pressed1 || pressed2;
        let all_released =
            (released1 && !self.stable_btn2) || (released2 && !self.stable_btn1);

        match self.state {
            State::Idle => {
                if any_pressed {
                    self.press_start = now;
                    self.was_active = false;
                    self.mic_on_at_press = input.mic_on;
                    self.physical_toggle = pressed1;
                    if !pressed1 {
                        click = true;
                    }
                    self.state = State::Pressing;
                }
            }
            State::Pressing => {
                if all_released {
                    if self.was_active && self.mic_on_at_press {
                        // Mic was already on when the retrigger press started:
                        // no toggle click, just restart the 10-s timer (TC-B2/B3).
                        // physical_toggle (btn1) always goes directly to Idle.
                        if self.physical_toggle {
                            self.state = State::Idle;
                        } else {
                            self.timer_start = now;
                            self.state = State::Timed;
                        }
                    } else {
                        // Mic was off (or first press): emit click, start timer.
                        if !self.physical_toggle {
                            // btn2 toggles: first click already sent on press;
                            // now emit the matching off-click so the cycle is
                            // always two clicks total (on + off via timer).
                            // But wait – for a normal short press the off is done
                            // by the timer later.  Here we only want a click when
                            // was_active=true AND mic_on=false (TC-B-toggle-off).
                            if self.was_active {
                                click = true; // toggle off while timed but mic is out
                            }
                        }
                        self.timer_start = now;
                        self.state = State::Timed;
                    }
                } else if now.wrapping_sub(self.press_start) >= HOLD_MS {
                    self.state = State::Held;
                }
            }
            State::Timed => {
                if pressed1 {
                    self.state = State::Idle;
                } else if pressed2 {
                    self.press_start = now;
                    self.was_active = true;
                    self.mic_on_at_press = input.mic_on;
                    self.physical_toggle = false;
                    self.state = State::Pressing;
                } else if now.wrapping_sub(self.timer_start) >= TIMER_MS {
                    click = true;
                    self.state = State::Idle;
                }
            }
            State::Held => {
                if pressed1 {
                    // Button 1 physically toggled mic off; suppress the
                    // release-click so we do not turn it back on again.
                    self.state = State::SuppressedUntilRelease;
                } else if all_released {
                    self.gap_start = now;
                    self.state = State::Gap;
                }
            }
            State::SuppressedUntilRelease => {
                // Wait until Button 2 is fully released, then go back to Idle
                // without emitting a click (mic is already off via Button 1).
                if !self.stable_btn2 {
                    self.state = State::Idle;
                }
            }
            State::Gap => {
                if now.wrapping_sub(self.gap_start) >= GAP_MS {
                    click = true;
                    self.state = State::Idle;
                }
            }
        }

        // Mic sync: correct mismatch after SYNC_MS tolerance
        let mic_should_be_on = matches!(self.state, State::Timed | State::Held | State::Gap);
        if input.mic_on != mic_should_be_on && !matches!(self.state, State::Pressing) {
            if !self.mismatch_active {
                self.mismatch_since = now;
                self.mismatch_active = true;
            } else if now.wrapping_sub(self.mismatch_since) >= SYNC_MS {
                click = true;
                self.mismatch_active = false;
            }
        } else {
            self.mismatch_active = false;
        }

        let led = match self.state {
            State::Idle => false,
            State::Pressing | State::Held | State::Gap => true,
            State::SuppressedUntilRelease => false,
            State::Timed => (now.wrapping_sub(self.timer_start) / BLINK_MS) % 2 == 0,
        };

        Output { click, led }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: u32 = DEBOUNCE_MS;

    fn inp(now: u32, btn1: bool, btn2: bool) -> Input {
        Input { now, btn1, btn2, mic_on: false }
    }

    fn inp_mic(now: u32, btn1: bool, btn2: bool, mic_on: bool) -> Input {
        Input { now, btn1, btn2, mic_on }
    }

    fn settle(ctrl: &mut Controller, t: u32, btn1: bool, btn2: bool) -> Output {
        ctrl.update(&inp(t, btn1, btn2));
        ctrl.update(&inp(t + D, btn1, btn2))
    }

    fn settle_mic(ctrl: &mut Controller, t: u32, btn1: bool, btn2: bool, mic_on: bool) -> Output {
        ctrl.update(&inp_mic(t, btn1, btn2, mic_on));
        ctrl.update(&inp_mic(t + D, btn1, btn2, mic_on))
    }

    #[test]
    fn initial_state_is_idle() {
        let ctrl = Controller::new();
        assert_eq!(ctrl.state, State::Idle);
    }

    #[test]
    fn idle_stays_idle() {
        let mut ctrl = Controller::new();
        let out = ctrl.update(&inp(0, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(!out.click);
        assert!(!out.led);
    }

    #[test]
    fn btn1_press_no_click() {
        let mut ctrl = Controller::new();
        let out = settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(!out.click);
    }

    #[test]
    fn btn2_press_sends_click() {
        let mut ctrl = Controller::new();
        let out = settle(&mut ctrl, 100, false, true);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(out.click);
    }

    #[test]
    fn short_press_to_timed() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        let out = settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);
        assert!(!out.click);
    }

    #[test]
    fn timed_expires_after_10s() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);

        let out = ctrl.update(&inp_mic(200 + D + TIMER_MS - 1, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        assert!(!out.click);

        let out = ctrl.update(&inp_mic(200 + D + TIMER_MS, false, false, true));
        assert_eq!(ctrl.state, State::Idle);
        assert!(out.click);
    }

    #[test]
    fn timed_btn1_turns_off() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        let out = settle(&mut ctrl, 1000, true, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(!out.click);
    }

    #[test]
    fn timed_btn2_turns_off_via_click() {
        // Originally tested btn2-in-Timed as a toggle-off.  Per the updated spec
        // (TC-B2) btn2 pressed while Timed re-triggers (restarts the 10-s timer)
        // without emitting a click – actual turn-off happens at timer expiry.
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        // At t=1000 mic is on (Timed) – simulate with mic_on=true so retrigger
        // logic applies (TC-B2).
        settle_mic(&mut ctrl, 1000, false, true, true);
        assert_eq!(ctrl.state, State::Pressing);
        let out = settle_mic(&mut ctrl, 1100, false, false, true);
        assert_eq!(ctrl.state, State::Timed, "retrigger should return to Timed");
        assert!(!out.click, "no extra click on retrigger");
    }

    #[test]
    fn long_press_to_held() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&inp(100 + D + HOLD_MS - 1, true, false));
        assert_eq!(ctrl.state, State::Pressing);
        ctrl.update(&inp(100 + D + HOLD_MS, true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    #[test]
    fn held_release_gap_idle() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&inp(100 + D + HOLD_MS, true, false));

        let out = settle(&mut ctrl, 800, false, false);
        assert_eq!(ctrl.state, State::Gap);
        assert!(!out.click);

        let out = ctrl.update(&inp(800 + D + GAP_MS - 1, false, false));
        assert!(!out.click);

        let out = ctrl.update(&inp(800 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(out.click);
    }

    #[test]
    fn both_buttons_no_click() {
        let mut ctrl = Controller::new();
        let out = settle(&mut ctrl, 100, true, true);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(!out.click);
    }

    #[test]
    fn held_needs_all_released() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, true);
        ctrl.update(&inp(100 + D + HOLD_MS, true, true));
        assert_eq!(ctrl.state, State::Held);
        settle(&mut ctrl, 800, false, true);
        assert_eq!(ctrl.state, State::Held);
        settle(&mut ctrl, 900, false, false);
        assert_eq!(ctrl.state, State::Gap);
    }

    #[test]
    fn debounce_ignores_bounce() {
        let mut ctrl = Controller::new();
        ctrl.update(&inp(100, true, false));
        ctrl.update(&inp(110, false, false));
        ctrl.update(&inp(115, true, false));
        ctrl.update(&inp(115 + D, true, false));
        assert_eq!(ctrl.state, State::Pressing);
    }

    #[test]
    fn short_glitch_ignored() {
        let mut ctrl = Controller::new();
        ctrl.update(&inp(100, true, false));
        ctrl.update(&inp(110, false, false));
        ctrl.update(&inp(110 + D, false, false));
        assert_eq!(ctrl.state, State::Idle);
    }

    #[test]
    fn led_blinks_in_timed() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);

        assert!(ctrl.update(&inp(200 + D, false, false)).led);
        assert!(!ctrl.update(&inp(200 + D + BLINK_MS, false, false)).led);
        assert!(ctrl.update(&inp(200 + D + 2 * BLINK_MS, false, false)).led);
    }

    #[test]
    fn timer_wrapping() {
        let mut ctrl = Controller::new();
        let t0 = u32::MAX - 200;
        settle(&mut ctrl, t0, true, false);
        settle(&mut ctrl, t0.wrapping_add(100), false, false);
        let out = ctrl.update(&inp(t0.wrapping_add(100 + D + TIMER_MS), false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(out.click);
    }

    #[test]
    fn btn1_short_cycle_one_click() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        if settle(&mut ctrl, 100, true, false).click { clicks += 1; }
        if settle(&mut ctrl, 200, false, false).click { clicks += 1; }
        if ctrl.update(&inp(200 + D + TIMER_MS, false, false)).click { clicks += 1; }
        assert_eq!(clicks, 1);
    }

    #[test]
    fn btn2_short_cycle_two_clicks() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        if settle(&mut ctrl, 100, false, true).click { clicks += 1; }
        if settle(&mut ctrl, 200, false, false).click { clicks += 1; }
        if ctrl.update(&inp(200 + D + TIMER_MS, false, false)).click { clicks += 1; }
        assert_eq!(clicks, 2);
    }

    #[test]
    fn btn1_hold_cycle_one_click() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        if settle(&mut ctrl, 100, true, false).click { clicks += 1; }
        if ctrl.update(&inp(100 + D + HOLD_MS, true, false)).click { clicks += 1; }
        if settle(&mut ctrl, 800, false, false).click { clicks += 1; }
        if ctrl.update(&inp(800 + D + GAP_MS, false, false)).click { clicks += 1; }
        assert_eq!(clicks, 1);
    }

    #[test]
    fn btn2_hold_cycle_two_clicks() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        if settle(&mut ctrl, 100, false, true).click { clicks += 1; }
        if ctrl.update(&inp(100 + D + HOLD_MS, false, true)).click { clicks += 1; }
        if settle(&mut ctrl, 800, false, false).click { clicks += 1; }
        if ctrl.update(&inp(800 + D + GAP_MS, false, false)).click { clicks += 1; }
        assert_eq!(clicks, 2);
    }

    #[test]
    fn sync_idle_mic_on_corrects() {
        let mut ctrl = Controller::new();
        ctrl.update(&inp_mic(0, false, false, true));
        let out = ctrl.update(&inp_mic(SYNC_MS, false, false, true));
        assert!(out.click);
    }

    #[test]
    fn sync_no_correction_when_matching() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);
        let out = ctrl.update(&inp_mic(200 + D + 1000, false, false, true));
        assert!(!out.click);
    }

    #[test]
    fn sync_timed_mic_off_corrects() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);
        ctrl.update(&inp_mic(300, false, false, false));
        let out = ctrl.update(&inp_mic(300 + SYNC_MS, false, false, false));
        assert!(out.click);
    }

    // ───────────────────────────── TC-A: Startup / Recovery ─────────────────

    /// TC-A1  MCU startet mit Mic aus → interner Zustand Idle, kein click
    #[test]
    fn tc_a1_startup_mic_off_is_idle() {
        let mut ctrl = Controller::new();
        let out = ctrl.update(&inp_mic(0, false, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(!out.click);
    }

    /// TC-A2  MCU startet mit Mic an → Sync-Mechanismus korrigiert nach SYNC_MS
    #[test]
    fn tc_a2_startup_mic_on_sync_corrects() {
        let mut ctrl = Controller::new();
        // Mic is already on at boot; controller is in Idle → mismatch
        ctrl.update(&inp_mic(0, false, false, true));
        let out = ctrl.update(&inp_mic(SYNC_MS, false, false, true));
        assert!(out.click, "should emit a click to turn mic off");
        // After correction the controller tracks the right state
    }

    // ─────────────────────────── TC-B: Kurzer Druck Button 2 ─────────────────

    /// TC-B1  Kurzdruck aus Idle: einschalten, 10-s-Timer, ausschalten
    #[test]
    fn tc_b1_short_press_from_idle_full_cycle() {
        let mut ctrl = Controller::new();
        // Press btn2 (simulates a virtual toggle-on)
        let out_press = settle_mic(&mut ctrl, 100, false, true, false);
        assert!(out_press.click, "press should emit click (turn on)");
        assert_eq!(ctrl.state, State::Pressing);

        // Release → Timed mode
        let out_rel = settle_mic(&mut ctrl, 200, false, false, true);
        assert!(!out_rel.click);
        assert_eq!(ctrl.state, State::Timed);

        // Timer fires after TIMER_MS
        let out_timer = ctrl.update(&inp_mic(200 + D + TIMER_MS, false, false, true));
        assert!(out_timer.click, "timer should emit click (turn off)");
        assert_eq!(ctrl.state, State::Idle);
    }

    /// TC-B2  Kurzdruck wenn Mic bereits an → kein Extra-Toggle, Timer reset
    #[test]
    fn tc_b2_short_press_while_already_on_no_extra_toggle() {
        let mut ctrl = Controller::new();
        // Reach Timed state via btn2 short press
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Retrigger: press at t=5000, stable at t=5000+D
        // press_start will be set to 5000+D when edge is detected in settle()
        let out2 = settle_mic(&mut ctrl, 5_000, false, true, true);
        assert!(!out2.click, "should not toggle again – mic is already on");
        assert_eq!(ctrl.state, State::Pressing);

        // Release early enough that after debounce settle the hold threshold is
        // not reached: release raw at press_start + HOLD_MS - D - 1
        // press_start = 5000+D, so release raw at 5000 + HOLD_MS - 1
        let press_start = 5_000 + D;
        let release_raw = press_start + HOLD_MS - D - 1;
        ctrl.update(&inp_mic(release_raw, false, false, true));
        ctrl.update(&inp_mic(release_raw + D, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
    }

    /// TC-B3  Retrigger innerhalb des 10-s-Timers verlängert Timer
    #[test]
    fn tc_b3_retrigger_extends_timer() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, false, true, false);
        // press_start = 100+D; release early enough: raw at 100+HOLD_MS-1
        let release1_raw = 100 + HOLD_MS - 1;
        ctrl.update(&inp_mic(release1_raw, false, false, false));
        ctrl.update(&inp_mic(release1_raw + D, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        // timer_start set at release1_raw+D
        let timer_start1 = release1_raw + D;

        // Re-press after 4 s (from stable release)
        let repress_raw = timer_start1 + 4_000;
        settle_mic(&mut ctrl, repress_raw, false, true, true);
        // press_start(2) = repress_raw+D; release early
        let release2_raw = repress_raw + HOLD_MS - 1;
        ctrl.update(&inp_mic(release2_raw, false, false, true));
        ctrl.update(&inp_mic(release2_raw + D, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        let timer_start2 = release2_raw + D;

        // Old timer position should NOT fire
        let old_fire = timer_start1 + TIMER_MS;
        // old_fire < timer_start2 + TIMER_MS, so state is still Timed
        let no_click = ctrl.update(&inp_mic(old_fire, false, false, true));
        assert!(!no_click.click, "old timer must not fire after retrigger");
        assert_eq!(ctrl.state, State::Timed);

        // New timer fires
        let out = ctrl.update(&inp_mic(timer_start2 + TIMER_MS, false, false, true));
        assert!(out.click, "new timer should fire");
        assert_eq!(ctrl.state, State::Idle);
    }

    /// TC-B4  Mehrfaches Retriggern: nur ein Toggle je Druckvorgang
    #[test]
    fn tc_b4_multiple_retriggers_no_extra_clicks() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;

        // Initial press: raw at 100, stable at 100+D
        if settle_mic(&mut ctrl, 100, false, true, false).click { clicks += 1; }
        // Release before hold threshold: raw at 100+HOLD_MS-1
        let r = 100 + HOLD_MS - 1;
        if ctrl.update(&inp_mic(r, false, false, false)).click { clicks += 1; }
        if ctrl.update(&inp_mic(r + D, false, false, true)).click { clicks += 1; }
        assert_eq!(ctrl.state, State::Timed);

        // Three retrigger presses – none should produce an additional click
        let mut t_base = r + D + 500;
        for _ in 0..3u32 {
            if settle_mic(&mut ctrl, t_base, false, true, true).click { clicks += 1; }
            // Release before hold threshold
            let rr = t_base + HOLD_MS - 1;
            if ctrl.update(&inp_mic(rr, false, false, true)).click { clicks += 1; }
            if ctrl.update(&inp_mic(rr + D, false, false, true)).click { clicks += 1; }
            assert_eq!(ctrl.state, State::Timed);
            t_base = rr + D + 500;
        }
        assert_eq!(clicks, 1, "only the initial on-click, no extra clicks during retrigger");
    }

    /// TC-B5  Druckdauer knapp unter Hold-Schwelle → Kurzdruck
    ///
    /// press_start is set when the stable edge fires (at raw_press + D).
    /// Release must arrive as a raw signal early enough that even after the
    /// debounce settle (release_raw + D) the duration is still < HOLD_MS.
    /// So: release_raw + D - press_start < HOLD_MS
    ///     → release_raw < press_start + HOLD_MS - D
    #[test]
    fn tc_b5_just_below_hold_threshold_is_short_press() {
        let mut ctrl = Controller::new();
        // raw press at 100 → stable (press_start) at 100+D
        settle(&mut ctrl, 100, false, true);
        let press_start = 100 + D;
        // Release raw 1 ms before the stable-release would hit HOLD_MS
        let release_raw = press_start + HOLD_MS - D - 1;
        ctrl.update(&inp(release_raw, false, false));
        ctrl.update(&inp(release_raw + D, false, false));
        assert_eq!(ctrl.state, State::Timed, "sub-threshold release must give short press");
    }

    // ─────────────────────────── TC-C: Langer Druck Button 2 ─────────────────

    /// TC-C1  Langer Druck aus Idle: ein Toggle beim Eintritt, ein Toggle beim Release
    #[test]
    fn tc_c1_long_press_from_idle_two_clicks() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        // btn2 pressed – first click on press
        if settle(&mut ctrl, 100, false, true).click { clicks += 1; }
        // hold past HOLD_MS
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);
        // Release → Gap
        if settle(&mut ctrl, 800, false, false).click { clicks += 1; }
        assert_eq!(ctrl.state, State::Gap);
        // Gap fires click
        if ctrl.update(&inp(800 + D + GAP_MS, false, false)).click { clicks += 1; }
        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(clicks, 2, "exactly two clicks: on and off");
    }

    /// TC-C2  Langer Druck bei Mic bereits an → kein Click beim Hold-Eintritt,
    ///        Click beim Release
    #[test]
    fn tc_c2_long_press_mic_already_on_no_entry_click() {
        let mut ctrl = Controller::new();
        // Reach Timed first
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // New long press while mic is on
        let out_press = settle_mic(&mut ctrl, 500, false, true, true);
        assert!(!out_press.click, "no click when pressing while already on");
        ctrl.update(&inp_mic(500 + D + HOLD_MS, false, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release → Gap → click (turn off)
        settle_mic(&mut ctrl, 1_200, false, false, true);
        assert_eq!(ctrl.state, State::Gap);
        let out_gap = ctrl.update(&inp_mic(1_200 + D + GAP_MS, false, false, true));
        assert!(out_gap.click, "should emit click to turn off on release");
    }

    /// TC-C3  Sehr langer Druck (30 s) – genau ein On, genau ein Off
    #[test]
    fn tc_c3_very_long_hold_exactly_two_clicks() {
        let mut ctrl = Controller::new();
        let mut clicks = 0u32;
        if settle_mic(&mut ctrl, 100, false, true, false).click { clicks += 1; }
        // press_start = 100+D; transition to Held one tick later
        let t_held = 100 + D + HOLD_MS;
        ctrl.update(&inp_mic(t_held, false, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Stay held for 30 s; mic_on=true so sync logic sees no mismatch
        for i in 1..30u32 {
            let out = ctrl.update(&inp_mic(t_held + i * 1_000, false, true, true));
            if out.click { clicks += 1; }
        }
        assert_eq!(ctrl.state, State::Held);

        // Raw release at 31_000; stable release at 31_000+D → Gap
        ctrl.update(&inp_mic(31_000, false, false, true));
        ctrl.update(&inp_mic(31_000 + D, false, false, true)); // → Gap
        assert_eq!(ctrl.state, State::Gap);

        let out_gap = ctrl.update(&inp_mic(31_000 + D + GAP_MS, false, false, true));
        if out_gap.click { clicks += 1; }
        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(clicks, 2, "exactly two clicks: press-on and gap-off");
    }

    /// TC-C4  Genau an Hold-Schwelle → Langdruck
    #[test]
    fn tc_c4_exactly_at_hold_threshold_is_long_press() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);
    }

    /// TC-C5  Knapp über Hold-Schwelle → Langdruck
    #[test]
    fn tc_c5_just_over_hold_threshold_is_long_press() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS + 1, false, true));
        assert_eq!(ctrl.state, State::Held);
    }

    // ────────────── TC-D: Button 1 während Hold (SuppressedUntilRelease) ─────

    /// TC-D1  Button 1 während Hold → Suppress, kein erneutes Toggle durch Release
    #[test]
    fn tc_d1_btn1_during_hold_suppresses_release_click() {
        let mut ctrl = Controller::new();
        // Enter Held
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Button 1 fires (mic toggled off physically)
        let out = settle(&mut ctrl, 700, true, true);
        assert_eq!(ctrl.state, State::SuppressedUntilRelease);
        assert!(!out.click, "no auto-click when btn1 acts during hold");
    }

    /// TC-D2  In SuppressedUntilRelease: weiteres Halten erzeugt keinen Click
    #[test]
    fn tc_d2_suppress_hold_no_extra_click() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        settle(&mut ctrl, 700, true, true); // → SuppressedUntilRelease
        assert_eq!(ctrl.state, State::SuppressedUntilRelease);

        for i in 0..10u32 {
            let out = ctrl.update(&inp(800 + i * 100, false, true));
            assert!(!out.click, "must not click while suppressed");
            assert_eq!(ctrl.state, State::SuppressedUntilRelease);
        }
    }

    /// TC-D3  Nach Suppress und Release → Idle, Mic bleibt aus
    #[test]
    fn tc_d3_suppress_release_goes_to_idle() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        settle(&mut ctrl, 700, true, true); // → SuppressedUntilRelease
        // Release btn2
        let out = settle(&mut ctrl, 900, false, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(!out.click, "no click on release after suppress");
    }

    /// TC-D4  Nach Suppress und Release: neuer Press funktioniert wieder normal
    #[test]
    fn tc_d4_after_suppress_new_press_works_normally() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        settle(&mut ctrl, 700, true, true);
        settle(&mut ctrl, 900, false, false); // back to Idle

        // New short press should work as expected
        let out = settle(&mut ctrl, 1_100, false, true);
        assert!(out.click, "new press after suppress should emit click");
        assert_eq!(ctrl.state, State::Pressing);
    }

    /// TC-D5  Button 1 mehrfach während Hold → bleibt suppressed, kein Re-On
    #[test]
    fn tc_d5_multiple_btn1_during_hold_stays_suppressed() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);

        // First btn1 press
        settle(&mut ctrl, 700, true, true);
        assert_eq!(ctrl.state, State::SuppressedUntilRelease);

        // Simulate additional btn1 presses – should stay suppressed, no click
        settle(&mut ctrl, 800, false, true);
        settle(&mut ctrl, 900, true, true);
        let out = settle(&mut ctrl, 1_000, false, true);
        assert_eq!(ctrl.state, State::SuppressedUntilRelease);
        assert!(!out.click);
    }

    // ─────────────────── TC-E: Button 1 während TimedOn ──────────────────────

    /// TC-E1  Button 1 schaltet Mic vorzeitig aus: Timer-Ablauf darf kein falsches
    ///        Toggle erzeugen
    #[test]
    fn tc_e1_btn1_turns_off_during_timed_no_false_toggle_at_timer_end() {
        let mut ctrl = Controller::new();
        // Enter Timed via btn2
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 physically turns mic off → controller goes Idle
        settle_mic(&mut ctrl, 1_000, true, false, false);
        assert_eq!(ctrl.state, State::Idle);

        // Old timer position passes – no click should fire
        let out = ctrl.update(&inp_mic(200 + D + TIMER_MS, false, false, false));
        assert!(!out.click, "timer must not fire after btn1 turned mic off");
    }

    /// TC-E2  Button 1 schaltet aus und wieder an während TimedOn: am Timer-Ende
    ///        sauber ausschalten
    #[test]
    fn tc_e2_btn1_off_then_on_during_timed_clean_off_at_end() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 turns mic off → Idle
        settle_mic(&mut ctrl, 1_000, true, false, false);
        assert_eq!(ctrl.state, State::Idle);

        // btn1 turns mic back on (e.g. direct Button 1 usage) – but controller
        // is Idle and mic_on now mismatches → sync corrects after SYNC_MS
        // This ensures no stale timer fires; the sync path handles it.
        ctrl.update(&inp_mic(1_100, false, false, true));
        let out_sync = ctrl.update(&inp_mic(1_100 + SYNC_MS, false, false, true));
        assert!(out_sync.click, "sync should correct the unexpected mic-on after idle");
    }

    /// TC-E3  Kurzdruck nach externer Abschaltung startet Timed neu
    #[test]
    fn tc_e3_short_press_after_external_off_restarts_timed() {
        let mut ctrl = Controller::new();
        // Enter Timed
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 cuts mic
        settle_mic(&mut ctrl, 1_000, true, false, false);
        assert_eq!(ctrl.state, State::Idle);

        // New btn2 press → Timed again
        let out = settle_mic(&mut ctrl, 2_000, false, true, false);
        assert!(out.click, "new press should turn on again");
        settle_mic(&mut ctrl, 2_100, false, false, true);
        assert_eq!(ctrl.state, State::Timed, "should be in Timed after restart");
    }

    // ─────────────────────────── TC-F: Debounce / Signalgrenzen ─────────────

    /// TC-F1  Prellen beim Drücken → genau ein Press-Ereignis
    #[test]
    fn tc_f1_bounce_on_press_single_event() {
        let mut ctrl = Controller::new();
        let mut presses = 0u32;
        let initial_state = ctrl.state;
        // Simulate bouncy signal: multiple rapid transitions before stable
        for &(t, v) in &[(100u32, true), (105, false), (112, true), (118, false), (125, true)] {
            ctrl.update(&inp(t, false, v));
        }
        // After DEBOUNCE_MS stable high
        let out = ctrl.update(&inp(125 + D, false, true));
        if out.click { presses += 1; }
        let out2 = ctrl.update(&inp(125 + D + 1, false, true));
        if out2.click { presses += 1; }
        assert_eq!(ctrl.state, State::Pressing);
        assert_eq!(presses, 1, "exactly one press event after debounce");
        let _ = initial_state;
    }

    /// TC-F2  Prellen beim Loslassen → genau ein Release-Ereignis
    #[test]
    fn tc_f2_bounce_on_release_single_event() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true); // stable pressed
        ctrl.update(&inp(100 + D + HOLD_MS, false, true)); // → Held

        // Bouncy release
        for &(t, v) in &[(800u32, false), (805, true), (812, false), (820, true), (828, false)] {
            ctrl.update(&inp(t, false, v));
        }
        ctrl.update(&inp(828 + D, false, false)); // stable released
        assert_eq!(ctrl.state, State::Gap, "should be in Gap after single release");
    }

    /// TC-F3  Kein Doppelauslösen: entweder Kurz- oder Langdruck, niemals beides
    #[test]
    fn tc_f3_no_double_trigger_at_hold_boundary() {
        // Short press: release raw such that stable-release is < HOLD_MS after press_start
        // press_start = 100+D; stable-release = release_raw+D; need that < press_start+HOLD_MS
        // → release_raw < 100 + HOLD_MS - 1  →  release_raw = 100 + HOLD_MS - 2
        let mut ctrl_short = Controller::new();
        settle(&mut ctrl_short, 100, false, true);
        let r = 100 + HOLD_MS - 2;
        ctrl_short.update(&inp(r, false, false));
        ctrl_short.update(&inp(r + D, false, false));
        assert_eq!(ctrl_short.state, State::Timed, "short press must result in Timed");

        // Long press: hold until at or past HOLD_MS → Held, then release
        let mut ctrl_long = Controller::new();
        settle(&mut ctrl_long, 100, false, true);
        ctrl_long.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl_long.state, State::Held, "long press must result in Held");
    }

    /// TC-F4  Sehr kurzer Störimpuls unter Debounce-Zeit → keine Aktion
    #[test]
    fn tc_f4_glitch_below_debounce_no_action() {
        let mut ctrl = Controller::new();
        // A pulse shorter than DEBOUNCE_MS
        ctrl.update(&inp(100, false, true));
        ctrl.update(&inp(100 + DEBOUNCE_MS - 1, false, false));
        ctrl.update(&inp(100 + DEBOUNCE_MS, false, false));
        assert_eq!(ctrl.state, State::Idle, "sub-debounce glitch must be ignored");
    }

    // ─────────────────────────── TC-G: Timer- und Ereignisgrenzen ────────────

    /// TC-G1  Timer läuft exakt ab (boundary: TIMER_MS - 1 vs TIMER_MS)
    #[test]
    fn tc_g1_timer_fires_at_exact_boundary() {
        let mut ctrl = Controller::new();
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        let t_ref = 200 + D;

        let no_click = ctrl.update(&inp_mic(t_ref + TIMER_MS - 1, false, false, true));
        assert!(!no_click.click, "must not fire one tick before deadline");
        assert_eq!(ctrl.state, State::Timed);

        let fires = ctrl.update(&inp_mic(t_ref + TIMER_MS, false, false, true));
        assert!(fires.click, "must fire exactly at deadline");
        assert_eq!(ctrl.state, State::Idle);
    }

    /// TC-G2  Neuer Kurzdruck genau am Timerende: deterministisch
    ///
    /// Because of debouncing, a `pressed2` edge requires DEBOUNCE_MS of stable
    /// signal before it is recognised.  We simulate: button goes high DEBOUNCE_MS
    /// before the timer fires so that the stable edge lands exactly at t_fire.
    #[test]
    fn tc_g2_new_press_at_timer_end_is_deterministic() {
        let mut ctrl = Controller::new();
        // Enter Timed; release 1: raw at 100+HOLD_MS-1
        settle_mic(&mut ctrl, 100, false, true, false);
        let r = 100 + HOLD_MS - 1;
        ctrl.update(&inp_mic(r, false, false, false));
        ctrl.update(&inp_mic(r + D, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        let timer_start = r + D; // timer_start set at the stable release
        let t_fire = timer_start + TIMER_MS;

        // New btn2 press goes raw D ticks before t_fire so stable edge arrives at t_fire
        ctrl.update(&inp_mic(t_fire - D, false, true, true));
        // At t_fire the stable edge fires → pressed2 detected BEFORE timer branch.
        // pressed2 → Pressing (was_active=true, no click)
        let out = ctrl.update(&inp_mic(t_fire, false, true, true));
        assert_eq!(ctrl.state, State::Pressing,
            "new press at timer boundary should win over timer expiry");
        assert!(!out.click, "no extra click: mic is already on (was_active=true)");
    }

    /// TC-G3  Release genau an Hold-Schwelle → Langdruck
    #[test]
    fn tc_g3_release_exactly_at_hold_threshold_is_long_press() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        // Hold for exactly HOLD_MS (transition to Held)
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);
        // Release
        settle(&mut ctrl, 100 + D + HOLD_MS + 50, false, false);
        assert_eq!(ctrl.state, State::Gap);
    }

    // ─────────────────────────── Invarianten ─────────────────────────────────

    /// INV-1  Kein Timer-Ausschaltimpuls wenn Mic bereits aus
    #[test]
    fn inv1_no_timer_off_when_mic_already_off() {
        let mut ctrl = Controller::new();
        // Enter Timed
        settle_mic(&mut ctrl, 100, false, true, false);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Mic gets turned off externally (btn1) before timer fires
        settle_mic(&mut ctrl, 1_000, true, false, false);
        assert_eq!(ctrl.state, State::Idle);

        // Timer position passes – mic is already off, controller is Idle
        // No click must be generated that would re-toggle
        for t in [200 + D + TIMER_MS, 200 + D + TIMER_MS + 1_000] {
            let out = ctrl.update(&inp_mic(t, false, false, false));
            assert!(!out.click,
                "timer must not fire and re-toggle mic that is already off (t={t})");
        }
    }

    /// INV-2  Nach externer Abschaltung durch Button 1 während Hold: kein
    ///        automatisches Wiederanschalten durch weiteres Halten
    #[test]
    fn inv2_no_auto_on_after_btn1_during_hold() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        ctrl.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Button 1 fires → SuppressedUntilRelease
        settle(&mut ctrl, 700, true, true);
        assert_eq!(ctrl.state, State::SuppressedUntilRelease);

        // Continue holding btn2 for a long time – never a click
        for i in 0..20u32 {
            let out = ctrl.update(&inp(800 + i * 500, false, true));
            assert!(!out.click,
                "must not auto-turn-on while suppressed (step {i})");
        }
    }

    /// INV-3  Pro Zustandswechsel nur minimal notwendige Anzahl Toggle-Impulse
    #[test]
    fn inv3_minimal_toggles_per_cycle() {
        // Short-press cycle: exactly 2 clicks (on + off)
        let mut ctrl = Controller::new();
        let mut n = 0u32;
        for t in 0..=(200 + D + TIMER_MS + 10) {
            if ctrl.update(&inp_mic(
                t,
                false,
                t >= 100 && t <= 200,
                t >= 200 && t < 200 + D + TIMER_MS,
            )).click { n += 1; }
        }
        assert_eq!(n, 2, "short-press cycle must produce exactly 2 clicks");
    }

    /// INV-4  Ein Tastenvorgang Button 2 löst genau einen Modus aus (kurz ODER lang)
    #[test]
    fn inv4_one_press_one_mode() {
        // Short press: release raw before stable-release crosses HOLD_MS
        // press_start = 100+D; release raw at 100+HOLD_MS-2 → stable at 100+HOLD_MS-2+D
        // duration = HOLD_MS - 2 < HOLD_MS → Timed
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, false, true);
        let r = 100 + HOLD_MS - 2;
        ctrl.update(&inp(r, false, false));
        ctrl.update(&inp(r + D, false, false));
        assert_eq!(ctrl.state, State::Timed, "short press must end in Timed, not Held");

        // Long press: hold until Held, release → Gap (never Timed)
        let mut ctrl2 = Controller::new();
        settle(&mut ctrl2, 100, false, true);
        ctrl2.update(&inp(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl2.state, State::Held, "long press must transition to Held");
        ctrl2.update(&inp(800, false, false));
        ctrl2.update(&inp(800 + D, false, false));
        assert_eq!(ctrl2.state, State::Gap, "long press release must go to Gap, not Timed");
    }

    // ──────────────────── ADC → mic_on Konvertierung ─────────────────────────

    /// Typischer Mic-off-Wert (≈ 2.7 V → raw ≈ 552) → false
    #[test]
    fn adc_mic_off_voltage_is_false() {
        assert!(!mic_on_from_adc(552));
    }

    /// Typischer Mic-on-Wert (≈ 3.7 V → raw ≈ 757) → true
    #[test]
    fn adc_mic_on_voltage_is_true() {
        assert!(mic_on_from_adc(757));
    }

    /// Exakt auf Schwelle (655) → false (strict greater-than)
    #[test]
    fn adc_exactly_at_threshold_is_false() {
        assert!(!mic_on_from_adc(ADC_MIC_THRESHOLD));
    }

    /// Einen Schritt über Schwelle (656) → true
    #[test]
    fn adc_one_above_threshold_is_true() {
        assert!(mic_on_from_adc(ADC_MIC_THRESHOLD + 1));
    }

    /// Minimalwert (0 V → raw = 0) → false
    #[test]
    fn adc_zero_is_false() {
        assert!(!mic_on_from_adc(0));
    }

    /// Maximalwert (5 V → raw = 1023) → true
    #[test]
    fn adc_max_is_true() {
        assert!(mic_on_from_adc(1023));
    }
}
