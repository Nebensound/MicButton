//! Hardware-independent state machine for the Mic Button Controller.
//!
//! All logic is decoupled from hardware and can be tested on the
//! host machine. The `Controller::update()` method takes inputs
//! and returns actions that `main.rs` applies to the hardware.

// ── Constants ──
pub const HOLD_MS: u32 = 500; // Hold threshold (>=500 ms = hold)
pub const TIMER_MS: u32 = 10_000; // 10 s mic timer
pub const CLICK_MS: u32 = 100; // GPIO pulse duration (click)
pub const SYNC_MS: u32 = 500; // Tolerance before mic status is corrected
pub const BLINK_MS: u32 = 500; // Blink interval for status LED
pub const STARTUP_BLINK_MS: u32 = 100; // Startup blink duration per phase
pub const DEBOUNCE_MS: u32 = 30; // Debounce window for button inputs
pub const GAP_MS: u32 = 200; // Delay after Held release before sending mic-off click

// ── Input ──

/// All inputs the controller needs per tick
pub struct ButtonInput {
    /// Current time in milliseconds
    pub now: u32,
    /// Button 1 pressed (true = pressed)
    pub btn1: bool,
    /// Button 2 pressed (true = pressed)
    pub btn2: bool,
    /// Mic hardware status (true = mic on)
    pub mic_on: bool,
}

// ── Output (Actions) ──

/// LED state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedState {
    Off,
    On,
    /// Blink phase: true = on, false = off
    Blink(bool),
}

/// An action returned by the controller
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Send mic click pulse (toggle)
    MicClick,
    /// Set LED state
    Led(LedState),
}

// ── State Machine ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Mic off, waiting for input
    Idle,
    /// Button currently pressed – short or long not yet determined
    Pressing,
    /// Mic on, 10 s timer running (short press)
    Timed,
    /// Mic on as long as button is held (long press)
    Held,
    /// Short delay after Held release before sending mic-off click
    Gap,
}

/// The controller holds the entire state machine state
pub struct Controller {
    pub state: State,
    press_start: u32,
    timer_start: u32,
    gap_start: u32,
    was_active: bool,
    /// True when the current press was initiated by btn1 (PB0),
    /// which physically toggles the mic on its own.
    physical_toggle: bool,
    // Debounced button states and raw tracking
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

    /// Processes a tick and returns up to 2 actions.
    ///
    /// Must be called as often as possible (main loop).
    pub fn update(&mut self, input: &ButtonInput) -> [Action; 2] {
        // Default: no MicClick action, LED is always set
        let mut actions = [Action::Led(LedState::Off), Action::Led(LedState::Off)];
        let mut action_idx = 0;

        let now = input.now;

        // ── Debounce ──
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

        // ── Detect edges (on debounced signals) ──
        let pressed1 = self.stable_btn1 && !self.last_btn1;
        let released1 = !self.stable_btn1 && self.last_btn1;
        let pressed2 = self.stable_btn2 && !self.last_btn2;
        let released2 = !self.stable_btn2 && self.last_btn2;
        self.last_btn1 = self.stable_btn1;
        self.last_btn2 = self.stable_btn2;

        let any_pressed = pressed1 || pressed2;
        let all_released =
            (released1 && !self.stable_btn2) || (released2 && !self.stable_btn1);

        // ── State Machine ──
        match self.state {
            State::Idle => {
                if any_pressed {
                    self.press_start = now;
                    self.was_active = false;
                    self.physical_toggle = pressed1;
                    if !pressed1 {
                        // btn2 only: PB2 doesn't physically toggle the mic,
                        // so we must send a firmware click on PB0.
                        actions[action_idx] = Action::MicClick;
                        action_idx += 1;
                    }
                    self.state = State::Pressing;
                }
            }

            State::Pressing => {
                if all_released {
                    if self.was_active {
                        if !self.physical_toggle {
                            // btn2 initiated: firmware must send click to turn off
                            actions[action_idx] = Action::MicClick;
                            action_idx += 1;
                        }
                        // btn1: physical press already toggled mic off
                        self.state = State::Idle;
                    } else {
                        self.timer_start = now;
                        self.state = State::Timed;
                    }
                } else if now.wrapping_sub(self.press_start) >= HOLD_MS {
                    self.state = State::Held;
                }
            }

            State::Timed => {
                if pressed1 {
                    // btn1: physical press toggles mic off → done
                    self.state = State::Idle;
                } else if pressed2 {
                    // btn2: no physical toggle, go to Pressing for turn-off
                    self.press_start = now;
                    self.was_active = true;
                    self.physical_toggle = false;
                    self.state = State::Pressing;
                } else if now.wrapping_sub(self.timer_start) >= TIMER_MS {
                    actions[action_idx] = Action::MicClick;
                    action_idx += 1;
                    self.state = State::Idle;
                }
            }

            State::Held => {
                if all_released {
                    self.gap_start = now;
                    self.state = State::Gap;
                }
            }

            State::Gap => {
                if now.wrapping_sub(self.gap_start) >= GAP_MS {
                    actions[action_idx] = Action::MicClick;
                    action_idx += 1;
                    self.state = State::Idle;
                }
            }
        }

        // ── Status LED ──
        let led = match self.state {
            State::Idle => LedState::Off,
            State::Pressing => LedState::On,
            State::Timed => {
                let phase = (now.wrapping_sub(self.timer_start) / BLINK_MS) % 2;
                LedState::Blink(phase == 0)
            }
            State::Held => LedState::On,
            State::Gap => LedState::On,
        };
        actions[action_idx] = Action::Led(led);

        // ── Mic status synchronization ──
        let mic_should_be_on = matches!(self.state, State::Timed | State::Held | State::Gap);
        let mic_actual = input.mic_on;

        if mic_actual != mic_should_be_on && !matches!(self.state, State::Pressing) {
            if !self.mismatch_active {
                self.mismatch_since = now;
                self.mismatch_active = true;
            } else if now.wrapping_sub(self.mismatch_since) >= SYNC_MS {
                // Correction needed – find free slot or overwrite LED
                if action_idx == 0 {
                    actions[0] = Action::MicClick;
                    actions[1] = Action::Led(led);
                } else {
                    // MicClick already in [0], LED in [1] – fine
                }
                self.mismatch_active = false;
            }
        } else {
            self.mismatch_active = false;
        }

        actions
    }
}

// ══════════════════════════════════════════════════════════════════════
// Tests – run on the host via `cargo test`
// ══════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand for debounce offset
    const D: u32 = DEBOUNCE_MS;

    /// Helper: create input with defaults (mic_on = false)
    fn input(now: u32, btn1: bool, btn2: bool) -> ButtonInput {
        ButtonInput {
            now,
            btn1,
            btn2,
            mic_on: false,
        }
    }

    fn input_with_mic(now: u32, btn1: bool, btn2: bool, mic_on: bool) -> ButtonInput {
        ButtonInput {
            now,
            btn1,
            btn2,
            mic_on,
        }
    }

    /// Settle a button state through the debounce window.
    /// Sends input at `t` (raw change) and at `t + DEBOUNCE_MS` (stable edge).
    /// Returns the actions from the settling tick.
    fn settle(ctrl: &mut Controller, t: u32, btn1: bool, btn2: bool) -> [Action; 2] {
        ctrl.update(&input(t, btn1, btn2));
        ctrl.update(&input(t + D, btn1, btn2))
    }

    fn settle_mic(
        ctrl: &mut Controller,
        t: u32,
        btn1: bool,
        btn2: bool,
        mic_on: bool,
    ) -> [Action; 2] {
        ctrl.update(&input_with_mic(t, btn1, btn2, mic_on));
        ctrl.update(&input_with_mic(t + D, btn1, btn2, mic_on))
    }

    /// Checks if an action list contains a MicClick
    fn has_mic_click(actions: &[Action]) -> bool {
        actions.iter().any(|a| matches!(a, Action::MicClick))
    }

    /// Returns the LED state from the actions
    fn led_from_actions(actions: &[Action]) -> Option<LedState> {
        actions.iter().find_map(|a| match a {
            Action::Led(s) => Some(*s),
            _ => None,
        })
    }

    // ── Basic state transitions ──

    #[test]
    fn initial_state_is_idle() {
        let ctrl = Controller::new();
        assert_eq!(ctrl.state, State::Idle);
    }

    #[test]
    fn idle_no_input_stays_idle() {
        let mut ctrl = Controller::new();
        let actions = ctrl.update(&input(0, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(!has_mic_click(&actions));
        assert_eq!(led_from_actions(&actions), Some(LedState::Off));
    }

    #[test]
    fn idle_button1_press_no_mic_click() {
        let mut ctrl = Controller::new();
        let actions = settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(
            !has_mic_click(&actions),
            "btn1 physically toggles mic – no firmware click needed"
        );
    }

    #[test]
    fn idle_button2_press_sends_mic_click() {
        let mut ctrl = Controller::new();
        let actions = settle(&mut ctrl, 100, false, true);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(
            has_mic_click(&actions),
            "btn2 needs firmware click to toggle mic"
        );
    }

    // ── Short press: Idle → Pressing → Timed → Idle ──

    #[test]
    fn short_press_transitions_to_timed() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        let actions = settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);
        assert!(!has_mic_click(&actions), "No additional click on release");
    }

    #[test]
    fn timed_expires_after_10s() {
        let mut ctrl = Controller::new();

        // Short press → Timed (timer_start = 200 + D)
        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Not yet expired
        let actions = ctrl.update(&input_with_mic(200 + D + TIMER_MS - 1, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        assert!(!has_mic_click(&actions));

        // Expired at timer_start + TIMER_MS
        let actions = ctrl.update(&input_with_mic(200 + D + TIMER_MS, false, false, true));
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            has_mic_click(&actions),
            "Mic should be turned off after 10 s"
        );
    }

    #[test]
    fn timed_btn1_press_turns_off_directly() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 press during Timed → physical toggle → Idle
        let actions = settle(&mut ctrl, 1000, true, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            !has_mic_click(&actions),
            "btn1 physically toggles mic off – no firmware click"
        );
    }

    #[test]
    fn timed_btn2_press_turns_off_via_firmware() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // btn2 press → Pressing (was_active=true)
        settle(&mut ctrl, 1000, false, true);
        assert_eq!(ctrl.state, State::Pressing);

        // Release → firmware click to turn off
        let actions = settle(&mut ctrl, 1100, false, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            has_mic_click(&actions),
            "btn2 needs firmware click to turn off"
        );
    }

    // ── Long press: Idle → Pressing → Held → Gap → Idle ──

    #[test]
    fn long_press_transitions_to_held() {
        let mut ctrl = Controller::new();

        // Press settles at 100 + D → press_start = 100 + D
        settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        // Not yet held
        ctrl.update(&input(100 + D + HOLD_MS - 1, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Held threshold reached
        ctrl.update(&input(100 + D + HOLD_MS, true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    #[test]
    fn held_release_goes_through_gap() {
        let mut ctrl = Controller::new();

        // Long press → Held
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&input(100 + D + HOLD_MS, true, false));
        assert_eq!(ctrl.state, State::Held);

        // Release → Gap (no MicClick yet)
        let actions = settle(&mut ctrl, 800, false, false);
        assert_eq!(ctrl.state, State::Gap);
        assert!(!has_mic_click(&actions), "No click on entering Gap");

        // Gap not finished yet
        let actions = ctrl.update(&input(800 + D + GAP_MS - 1, false, false));
        assert_eq!(ctrl.state, State::Gap);
        assert!(!has_mic_click(&actions));

        // Gap finished → MicClick + Idle
        let actions = ctrl.update(&input(800 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            has_mic_click(&actions),
            "Mic should be turned off after gap"
        );
    }

    // ── Both buttons ──

    #[test]
    fn both_buttons_trigger_pressing_no_click() {
        let mut ctrl = Controller::new();
        let actions = settle(&mut ctrl, 100, true, true);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(
            !has_mic_click(&actions),
            "btn1 is included → physical toggle, no firmware click"
        );
    }

    #[test]
    fn held_needs_all_released() {
        let mut ctrl = Controller::new();

        // Press both → Held
        settle(&mut ctrl, 100, true, true);
        ctrl.update(&input(100 + D + HOLD_MS, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release btn1 only, btn2 still held → stays Held
        settle(&mut ctrl, 800, false, true);
        assert_eq!(ctrl.state, State::Held);

        // Release btn2 too → Gap
        settle(&mut ctrl, 900, false, false);
        assert_eq!(ctrl.state, State::Gap);

        // Gap expires → Idle + MicClick
        let actions = ctrl.update(&input(900 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    #[test]
    fn button2_alone_works_for_short_press() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, false, true);
        assert_eq!(ctrl.state, State::Pressing);

        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);
    }

    #[test]
    fn button_handoff_btn1_to_btn2_keeps_held() {
        let mut ctrl = Controller::new();

        // Press btn1 → Held
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&input(100 + D + HOLD_MS, true, false));
        assert_eq!(ctrl.state, State::Held);

        // Press btn2 additionally
        settle(&mut ctrl, 800, true, true);
        assert_eq!(ctrl.state, State::Held);

        // Release btn1, btn2 still held → stays Held
        settle(&mut ctrl, 900, false, true);
        assert_eq!(ctrl.state, State::Held);

        // Release btn2 → Gap
        settle(&mut ctrl, 1000, false, false);
        assert_eq!(ctrl.state, State::Gap);

        // Gap expires → Idle + MicClick
        let actions = ctrl.update(&input(1000 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    // ── LED behavior ──

    #[test]
    fn led_off_in_idle() {
        let mut ctrl = Controller::new();
        let actions = ctrl.update(&input(0, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Off));
    }

    #[test]
    fn led_on_in_pressing() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        let actions = ctrl.update(&input(100 + D + 50, true, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::On));
    }

    #[test]
    fn led_on_in_held() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&input(100 + D + HOLD_MS, true, false));
        let actions = ctrl.update(&input(100 + D + HOLD_MS + 100, true, false));
        assert_eq!(ctrl.state, State::Held);
        assert_eq!(led_from_actions(&actions), Some(LedState::On));
    }

    #[test]
    fn led_blinks_in_timed() {
        let mut ctrl = Controller::new();

        // Short press → Timed (timer_start = 200 + D)
        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // Phase 0 (0–499 ms after timer_start): on
        let actions = ctrl.update(&input(200 + D, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(true)));

        // Phase 1 (500–999 ms after timer_start): off
        let actions = ctrl.update(&input(200 + D + BLINK_MS, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(false)));

        // Phase 0 (1000–1499 ms after timer_start): on
        let actions = ctrl.update(&input(200 + D + 2 * BLINK_MS, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(true)));
    }

    #[test]
    fn led_on_in_gap() {
        let mut ctrl = Controller::new();
        settle(&mut ctrl, 100, true, false);
        ctrl.update(&input(100 + D + HOLD_MS, true, false));
        assert_eq!(ctrl.state, State::Held);
        settle(&mut ctrl, 800, false, false);
        assert_eq!(ctrl.state, State::Gap);

        let actions = ctrl.update(&input(800 + D + 50, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::On));
    }

    // ── Debouncing ──

    #[test]
    fn bounce_ignored_within_debounce_window() {
        let mut ctrl = Controller::new();

        // Button press with bouncing
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(110, false, false)); // bounce off
        ctrl.update(&input(115, true, false)); // bounce on

        // Debounce window from last change (115): settles at 115 + D
        ctrl.update(&input(115 + D, true, false));
        assert_eq!(
            ctrl.state,
            State::Pressing,
            "Debounced press should register"
        );
    }

    #[test]
    fn short_glitch_ignored() {
        let mut ctrl = Controller::new();

        // Very brief press that releases before debounce settles
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(110, false, false));

        // Wait past debounce for the release (false settled)
        ctrl.update(&input(110 + D, false, false));
        assert_eq!(
            ctrl.state,
            State::Idle,
            "Glitch shorter than debounce should be ignored"
        );
    }

    #[test]
    fn debounce_does_not_delay_stable_signal() {
        let mut ctrl = Controller::new();

        // Clean press – settles after exactly DEBOUNCE_MS
        ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Idle, "Not yet settled");
        ctrl.update(&input(100 + D, true, false));
        assert_eq!(ctrl.state, State::Pressing, "Settled after DEBOUNCE_MS");
    }

    // ── Mic status synchronization ──

    #[test]
    fn no_sync_correction_during_pressing() {
        let mut ctrl = Controller::new();

        settle_mic(&mut ctrl, 100, true, false, false);
        assert_eq!(ctrl.state, State::Pressing);

        // Mic reports "off" even though we just clicked → no correction
        for t in ((100 + D + 50)..(100 + D + 1000)).step_by(100) {
            let actions = ctrl.update(&input_with_mic(t, true, false, false));
            assert!(
                !has_mic_click(&actions),
                "No correction during Pressing at t={}",
                t
            );
        }
    }

    #[test]
    fn sync_corrects_after_tolerance() {
        let mut ctrl = Controller::new();

        // Short press → Timed
        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Mismatch starts
        let t_mis = 300 + D;
        ctrl.update(&input_with_mic(t_mis, false, false, false));

        // Still within tolerance
        ctrl.update(&input_with_mic(t_mis + SYNC_MS - 1, false, false, false));

        // Tolerance exceeded → correction click
        let actions = ctrl.update(&input_with_mic(t_mis + SYNC_MS, false, false, false));
        assert_eq!(ctrl.state, State::Timed);
        assert!(
            has_mic_click(&actions),
            "Correction click after 500ms mismatch"
        );
    }

    #[test]
    fn sync_no_correction_if_status_matches() {
        let mut ctrl = Controller::new();

        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Mic correctly reports "on" → no correction
        for t in ((200 + D + 100)..(200 + D + 5000)).step_by(100) {
            let actions = ctrl.update(&input_with_mic(t, false, false, true));
            assert!(
                !has_mic_click(&actions),
                "No correction when status is correct at t={}",
                t
            );
        }
    }

    #[test]
    fn sync_resets_on_brief_glitch() {
        let mut ctrl = Controller::new();

        settle_mic(&mut ctrl, 100, true, false, true);
        settle_mic(&mut ctrl, 200, false, false, true);

        // Mismatch begins
        let t_start = 300 + D;
        ctrl.update(&input_with_mic(t_start, false, false, false));
        ctrl.update(&input_with_mic(t_start + 200, false, false, false));

        // Glitch: status briefly returns → resets timer
        ctrl.update(&input_with_mic(t_start + 300, false, false, true));

        // New mismatch
        let t_new = t_start + 400;
        ctrl.update(&input_with_mic(t_new, false, false, false));
        let actions = ctrl.update(&input_with_mic(t_new + SYNC_MS - 1, false, false, false));
        assert!(
            !has_mic_click(&actions),
            "Tolerance timer was reset by glitch"
        );

        let actions = ctrl.update(&input_with_mic(t_new + SYNC_MS, false, false, false));
        assert!(has_mic_click(&actions), "Correction after reset + 500ms");
    }

    #[test]
    fn sync_idle_mic_on_corrects() {
        let mut ctrl = Controller::new();

        // Idle, but mic reports "on" → correct after tolerance
        ctrl.update(&input_with_mic(0, false, false, true));
        ctrl.update(&input_with_mic(SYNC_MS - 1, false, false, true));

        let actions = ctrl.update(&input_with_mic(SYNC_MS - 1, false, false, true));
        assert!(!has_mic_click(&actions));

        let actions = ctrl.update(&input_with_mic(SYNC_MS, false, false, true));
        assert!(has_mic_click(&actions), "Mic should be corrected in Idle");
    }

    // ── Wrapping / Overflow ──

    #[test]
    fn timer_handles_u32_wrapping() {
        let mut ctrl = Controller::new();

        let t0 = u32::MAX - 200;
        settle(&mut ctrl, t0, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        // Release → Timed (timer_start = t0 + 100 + D)
        settle(&mut ctrl, t0.wrapping_add(100), false, false);
        assert_eq!(ctrl.state, State::Timed);

        // Timer expiry across overflow
        let timer_start = t0.wrapping_add(100 + D);
        let actions = ctrl.update(&input(timer_start.wrapping_add(TIMER_MS), false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions), "Timer expiry across u32 overflow");
    }

    #[test]
    fn hold_detection_works_at_overflow() {
        let mut ctrl = Controller::new();

        let t0 = u32::MAX - 200;
        settle(&mut ctrl, t0, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        // press_start = t0 + D, hold at t0 + D + HOLD_MS
        ctrl.update(&input(t0.wrapping_add(D + HOLD_MS), true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    // ── Double press / re-press ──

    #[test]
    fn btn1_double_press_in_timed_goes_idle() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 press → physical toggle off → Idle
        let actions = settle(&mut ctrl, 1000, true, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(!has_mic_click(&actions));
    }

    #[test]
    fn btn2_double_press_in_timed_turns_off() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, false, true);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        settle(&mut ctrl, 1000, false, true);
        assert_eq!(ctrl.state, State::Pressing);

        let actions = settle(&mut ctrl, 1100, false, false);
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    #[test]
    fn btn1_long_press_during_timed_goes_idle() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // btn1 press → physical toggle off → Idle immediately
        settle(&mut ctrl, 1000, true, false);
        assert_eq!(ctrl.state, State::Idle);
    }

    #[test]
    fn btn2_long_press_during_timed_transitions_to_held() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, false, true);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        settle(&mut ctrl, 1000, false, true);
        assert_eq!(ctrl.state, State::Pressing);

        // press_start = 1000 + D, hold at 1000 + D + HOLD_MS
        ctrl.update(&input(1000 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);
    }

    // ── Edge detection ──

    #[test]
    fn held_button_no_repeated_trigger() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        // Keep holding – should transition to Held, never re-trigger MicClick
        for t in ((100 + D + 100)..5000).step_by(100) {
            ctrl.update(&input(t, true, false));
        }
        assert_eq!(ctrl.state, State::Held);
    }

    #[test]
    fn rapid_press_release_works() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        // timer_start = 200 + D, expires at 200 + D + TIMER_MS
        let actions = ctrl.update(&input(200 + D + TIMER_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    // ── Mic click counter ──

    #[test]
    fn btn1_short_press_cycle_produces_one_click() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = settle(&mut ctrl, 200, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        // timer_start = 200 + D
        let a = ctrl.update(&input(200 + D + TIMER_MS, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 1, "Only 1 firmware click: timer-off");
    }

    #[test]
    fn btn2_short_press_cycle_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = settle(&mut ctrl, 200, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = ctrl.update(&input(200 + D + TIMER_MS, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 2, "2 clicks: firmware-on, firmware-off");
    }

    #[test]
    fn btn1_hold_cycle_produces_one_click() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = ctrl.update(&input(100 + D + HOLD_MS, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Release → Gap (no click)
        let a = settle(&mut ctrl, 800, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Gap);

        // Gap expires → 1 click
        let a = ctrl.update(&input(800 + D + GAP_MS, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 1, "Only 1 firmware click: gap-off");
    }

    #[test]
    fn btn2_hold_cycle_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = ctrl.update(&input(100 + D + HOLD_MS, false, true));
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = settle(&mut ctrl, 800, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Gap);

        let a = ctrl.update(&input(800 + D + GAP_MS, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 2, "2 clicks: firmware-on, gap-off");
    }

    #[test]
    fn btn1_toggle_off_in_timed_produces_zero_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        settle(&mut ctrl, 200, false, false);

        let a = settle(&mut ctrl, 1000, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(
            click_count, 0,
            "Zero firmware clicks: btn1 physically toggles on and off"
        );
    }

    #[test]
    fn btn2_toggle_off_in_timed_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }
        settle(&mut ctrl, 200, false, false);

        let a = settle(&mut ctrl, 1000, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }

        let a = settle(&mut ctrl, 1100, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(
            click_count, 2,
            "2 firmware clicks: on (btn2 press), off (btn2 release)"
        );
    }

    // ── Sync correction in Held state ──

    #[test]
    fn sync_corrects_in_held_state() {
        let mut ctrl = Controller::new();

        settle_mic(&mut ctrl, 100, true, false, true);
        ctrl.update(&input_with_mic(100 + D + HOLD_MS, true, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Mic suddenly reports "off" while Held (should be on)
        let t_mis = 100 + D + HOLD_MS + 100;
        ctrl.update(&input_with_mic(t_mis, true, false, false));

        let actions = ctrl.update(&input_with_mic(t_mis + SYNC_MS - 1, true, false, false));
        assert!(!has_mic_click(&actions), "Still within tolerance");

        let actions = ctrl.update(&input_with_mic(t_mis + SYNC_MS, true, false, false));
        assert!(
            has_mic_click(&actions),
            "Correction click in Held state after 500ms mismatch"
        );
    }

    // ── Button 2 long press → Held ──

    #[test]
    fn button2_long_press_transitions_to_held() {
        let mut ctrl = Controller::new();

        let actions = settle(&mut ctrl, 100, false, true);
        assert_eq!(ctrl.state, State::Pressing);
        assert!(has_mic_click(&actions));

        ctrl.update(&input(100 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Release → Gap
        settle(&mut ctrl, 800, false, false);
        assert_eq!(ctrl.state, State::Gap);

        // Gap → Idle + MicClick
        let actions = ctrl.update(&input(800 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    // ── Timed → long press → Held → release ──

    #[test]
    fn btn2_timed_then_long_repress_to_held_and_release() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        let a = settle(&mut ctrl, 1000, false, true);
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Pressing);

        ctrl.update(&input(1000 + D + HOLD_MS, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Release → Gap
        let a = settle(&mut ctrl, 1700, false, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Gap);

        // Gap → Idle + MicClick
        let a = ctrl.update(&input(1700 + D + GAP_MS, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(
            click_count, 2,
            "2 firmware clicks: on (btn2 press), off (gap)"
        );
    }

    #[test]
    fn btn1_timed_repress_goes_idle() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        let a = settle(&mut ctrl, 100, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);

        let a = settle(&mut ctrl, 1000, true, false);
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(
            click_count, 0,
            "Zero firmware clicks: btn1 toggles physically both times"
        );
    }

    // ── Button handoff in Pressing state ──

    #[test]
    fn pressing_btn1_then_add_btn2_then_release_btn1_stays_pressing() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, false);
        assert_eq!(ctrl.state, State::Pressing);

        // Also press btn2
        settle(&mut ctrl, 200, true, true);
        assert_eq!(ctrl.state, State::Pressing);

        // Release btn1, btn2 still held → stays Pressing
        settle(&mut ctrl, 300, false, true);
        assert_eq!(ctrl.state, State::Pressing);

        // Release btn2 → Timed
        settle(&mut ctrl, 400, false, false);
        assert_eq!(ctrl.state, State::Timed);
    }

    // ── Sync wrapping overflow ──

    #[test]
    fn sync_mismatch_timer_handles_wrapping() {
        let mut ctrl = Controller::new();

        let t0 = u32::MAX - 300;
        settle_mic(&mut ctrl, t0, true, false, true);
        settle_mic(&mut ctrl, t0.wrapping_add(100), false, false, true);
        assert_eq!(ctrl.state, State::Timed);

        // Mismatch starts
        let t_mis = t0.wrapping_add(200);
        ctrl.update(&input_with_mic(t_mis, false, false, false));

        let actions =
            ctrl.update(&input_with_mic(t_mis.wrapping_add(SYNC_MS), false, false, false));
        assert!(
            has_mic_click(&actions),
            "Sync correction works across u32 overflow"
        );
    }

    // ── Simultaneous press + release both buttons ──

    #[test]
    fn both_buttons_simultaneous_release_in_pressing() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, true);
        assert_eq!(ctrl.state, State::Pressing);

        settle(&mut ctrl, 200, false, false);
        assert_eq!(ctrl.state, State::Timed);
    }

    #[test]
    fn both_buttons_simultaneous_release_in_held() {
        let mut ctrl = Controller::new();

        settle(&mut ctrl, 100, true, true);
        ctrl.update(&input(100 + D + HOLD_MS, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release both → Gap
        settle(&mut ctrl, 800, false, false);
        assert_eq!(ctrl.state, State::Gap);

        // Gap → Idle + MicClick
        let actions = ctrl.update(&input(800 + D + GAP_MS, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }
}
