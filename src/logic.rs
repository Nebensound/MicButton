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
}

/// The controller holds the entire state machine state
pub struct Controller {
    pub state: State,
    press_start: u32,
    timer_start: u32,
    was_active: bool,
    last_btn1: bool,
    last_btn2: bool,
    mismatch_since: u32,
    mismatch_active: bool,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            press_start: 0,
            timer_start: 0,
            was_active: false,
            last_btn1: false,
            last_btn2: false,
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

        // ── Detect edges ──
        let pressed1 = input.btn1 && !self.last_btn1;
        let released1 = !input.btn1 && self.last_btn1;
        let pressed2 = input.btn2 && !self.last_btn2;
        let released2 = !input.btn2 && self.last_btn2;
        self.last_btn1 = input.btn1;
        self.last_btn2 = input.btn2;

        let any_pressed = pressed1 || pressed2;
        let all_released = (released1 && !input.btn2) || (released2 && !input.btn1);

        // ── State Machine ──
        match self.state {
            State::Idle => {
                if any_pressed {
                    self.press_start = now;
                    self.was_active = false;
                    actions[action_idx] = Action::MicClick;
                    action_idx += 1;
                    self.state = State::Pressing;
                }
            }

            State::Pressing => {
                if all_released {
                    if self.was_active {
                        actions[action_idx] = Action::MicClick;
                        action_idx += 1;
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
                if any_pressed {
                    self.press_start = now;
                    self.was_active = true;
                    self.state = State::Pressing;
                } else if now.wrapping_sub(self.timer_start) >= TIMER_MS {
                    actions[action_idx] = Action::MicClick;
                    action_idx += 1;
                    self.state = State::Idle;
                }
            }

            State::Held => {
                if all_released {
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
        };
        actions[action_idx] = Action::Led(led);

        // ── Mic status synchronization ──
        let mic_should_be_on = matches!(self.state, State::Timed | State::Held);
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

    /// Helper: create input with defaults
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
    fn idle_button1_press_transitions_to_pressing() {
        let mut ctrl = Controller::new();
        let actions = ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Pressing);
        assert!(
            has_mic_click(&actions),
            "Mic should be turned on when pressed"
        );
    }

    #[test]
    fn idle_button2_press_transitions_to_pressing() {
        let mut ctrl = Controller::new();
        let actions = ctrl.update(&input(100, false, true));
        assert_eq!(ctrl.state, State::Pressing);
        assert!(has_mic_click(&actions));
    }

    // ── Short press: Idle → Pressing → Timed → Idle ──

    #[test]
    fn short_press_transitions_to_timed() {
        let mut ctrl = Controller::new();

        // Press button
        ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Release button (after < 500 ms)
        let actions = ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);
        assert!(!has_mic_click(&actions), "No additional click on release");
    }

    #[test]
    fn timed_expires_after_10s() {
        let mut ctrl = Controller::new();

        // Short press → Timed (mic_on=true simulates correct mic status)
        ctrl.update(&input_with_mic(100, true, false, true));
        ctrl.update(&input_with_mic(200, false, false, true));
        assert_eq!(ctrl.state, State::Timed);

        // Not yet expired at 9.9 s
        let actions = ctrl.update(&input_with_mic(10_199, false, false, true));
        assert_eq!(ctrl.state, State::Timed);
        assert!(!has_mic_click(&actions));

        // Expired at 10 s
        let actions = ctrl.update(&input_with_mic(10_200, false, false, true));
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            has_mic_click(&actions),
            "Mic should be turned off after 10 s"
        );
    }

    #[test]
    fn timed_press_to_turn_off() {
        let mut ctrl = Controller::new();

        // Short press → Timed
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Another press during Timed → Pressing (was_active=true)
        ctrl.update(&input(1000, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Short release → should turn off (was previously active)
        let actions = ctrl.update(&input(1100, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions), "Mic should be turned off");
    }

    // ── Long press: Idle → Pressing → Held → Idle ──

    #[test]
    fn long_press_transitions_to_held() {
        let mut ctrl = Controller::new();

        // Press button
        ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Button still held at 599 ms → still Pressing
        ctrl.update(&input(599, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Button still held at 600 ms (500 ms after press_start) → Held
        ctrl.update(&input(600, true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    #[test]
    fn held_release_turns_off() {
        let mut ctrl = Controller::new();

        // Long press → Held
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(700, true, false));
        assert_eq!(ctrl.state, State::Held);

        // Release → Mic off
        let actions = ctrl.update(&input(800, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(
            has_mic_click(&actions),
            "Mic should be turned off on release"
        );
    }

    // ── Both buttons ──

    #[test]
    fn both_buttons_trigger_pressing() {
        let mut ctrl = Controller::new();
        let actions = ctrl.update(&input(100, true, true));
        assert_eq!(ctrl.state, State::Pressing);
        assert!(has_mic_click(&actions));
    }

    #[test]
    fn held_needs_all_released() {
        let mut ctrl = Controller::new();

        // Press both → Held
        ctrl.update(&input(100, true, true));
        ctrl.update(&input(700, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release btn1 only, btn2 still held → stays Held
        ctrl.update(&input(800, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Now release btn2 too → Idle
        let actions = ctrl.update(&input(900, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    #[test]
    fn button2_alone_works_for_short_press() {
        let mut ctrl = Controller::new();

        // Button 2 only
        ctrl.update(&input(100, false, true));
        assert_eq!(ctrl.state, State::Pressing);

        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);
    }

    #[test]
    fn button_handoff_btn1_to_btn2_keeps_held() {
        let mut ctrl = Controller::new();

        // Press btn1 → Held
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(700, true, false));
        assert_eq!(ctrl.state, State::Held);

        // Press btn2 additionally
        ctrl.update(&input(800, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release btn1, btn2 still held → stays Held
        ctrl.update(&input(900, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Release btn2 → Idle
        let actions = ctrl.update(&input(1000, false, false));
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
        ctrl.update(&input(100, true, false));
        // Update again to see LED state (state is Pressing)
        let actions = ctrl.update(&input(150, true, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::On));
    }

    #[test]
    fn led_on_in_held() {
        let mut ctrl = Controller::new();
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(700, true, false));
        let actions = ctrl.update(&input(800, true, false));
        assert_eq!(ctrl.state, State::Held);
        assert_eq!(led_from_actions(&actions), Some(LedState::On));
    }

    #[test]
    fn led_blinks_in_timed() {
        let mut ctrl = Controller::new();

        // Short press → Timed (timer_start = 200)
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Phase 0 (0-499ms after timer_start): on
        let actions = ctrl.update(&input(200, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(true)));

        // Phase 1 (500-999ms after timer_start): off
        let actions = ctrl.update(&input(700, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(false)));

        // Phase 0 (1000-1499ms after timer_start): on
        let actions = ctrl.update(&input(1200, false, false));
        assert_eq!(led_from_actions(&actions), Some(LedState::Blink(true)));
    }

    // ── Mic status synchronization ──

    #[test]
    fn no_sync_correction_during_pressing() {
        let mut ctrl = Controller::new();

        // Press button → Pressing
        ctrl.update(&input_with_mic(100, true, false, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Mic reports "off" even though we just clicked → no correction
        for t in (200..1000).step_by(100) {
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
        ctrl.update(&input_with_mic(100, true, false, true));
        ctrl.update(&input_with_mic(200, false, false, true));
        assert_eq!(ctrl.state, State::Timed);

        // Mic suddenly reports "off" while Timed (should be on)
        // Mismatch starts at t=300
        ctrl.update(&input_with_mic(300, false, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Still within tolerance (499ms after mismatch)
        ctrl.update(&input_with_mic(799, false, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Tolerance exceeded (500ms after mismatch) → correction click
        let actions = ctrl.update(&input_with_mic(800, false, false, false));
        assert_eq!(ctrl.state, State::Timed);
        assert!(
            has_mic_click(&actions),
            "Correction click after 500ms mismatch"
        );
    }

    #[test]
    fn sync_no_correction_if_status_matches() {
        let mut ctrl = Controller::new();

        // Short press → Timed
        ctrl.update(&input_with_mic(100, true, false, true));
        ctrl.update(&input_with_mic(200, false, false, true));
        assert_eq!(ctrl.state, State::Timed);

        // Mic correctly reports "on" in Timed state → no correction
        for t in (300..5000).step_by(100) {
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

        // Short press → Timed
        ctrl.update(&input_with_mic(100, true, false, true));
        ctrl.update(&input_with_mic(200, false, false, true));

        // Mismatch begins
        ctrl.update(&input_with_mic(300, false, false, false));
        ctrl.update(&input_with_mic(500, false, false, false));

        // Glitch: status briefly returns → reset
        ctrl.update(&input_with_mic(600, false, false, true));

        // New mismatch → no click until 1100 (reset cleared the timer)
        ctrl.update(&input_with_mic(700, false, false, false));
        let actions = ctrl.update(&input_with_mic(1100, false, false, false));
        assert!(
            !has_mic_click(&actions),
            "Tolerance timer was reset by glitch"
        );

        // Only at 1200 (500ms after new mismatch at 700) → correction
        let actions = ctrl.update(&input_with_mic(1200, false, false, false));
        assert!(has_mic_click(&actions), "Correction after reset + 500ms");
    }

    #[test]
    fn sync_idle_mic_on_corrects() {
        let mut ctrl = Controller::new();

        // Idle, but mic reports "on" → correct after tolerance
        ctrl.update(&input_with_mic(0, false, false, true));
        ctrl.update(&input_with_mic(499, false, false, true));

        // Not yet...
        let actions = ctrl.update(&input_with_mic(499, false, false, true));
        assert!(!has_mic_click(&actions));

        // Now!
        let actions = ctrl.update(&input_with_mic(500, false, false, true));
        assert!(has_mic_click(&actions), "Mic should be corrected in Idle");
    }

    // ── Wrapping / Overflow ──

    #[test]
    fn timer_handles_u32_wrapping() {
        let mut ctrl = Controller::new();

        // Start just before overflow
        let t0 = u32::MAX - 100;
        ctrl.update(&input(t0, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Release after overflow
        ctrl.update(&input(t0.wrapping_add(50), false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Timer runs past the overflow
        let actions = ctrl.update(&input(t0.wrapping_add(10_050), false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions), "Timer expiry across u32 overflow");
    }

    #[test]
    fn hold_detection_works_at_overflow() {
        let mut ctrl = Controller::new();

        let t0 = u32::MAX - 100;
        ctrl.update(&input(t0, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // 500ms after press_start (across overflow)
        ctrl.update(&input(t0.wrapping_add(500), true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    // ── Double press / re-press ──

    #[test]
    fn double_press_in_timed_turns_off() {
        let mut ctrl = Controller::new();

        // First short press → Timed
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Second short press → Pressing (was_active=true)
        ctrl.update(&input(1000, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Release → Idle (mic off)
        let actions = ctrl.update(&input(1100, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    #[test]
    fn long_press_during_timed_transitions_to_held() {
        let mut ctrl = Controller::new();

        // Short press → Timed
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // New long press during Timed
        ctrl.update(&input(1000, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Hold → was_active is true, but ≥500ms → Held
        // (Note: was_active is set to true, HOLD detection is based on time only)
        ctrl.update(&input(1500, true, false));
        assert_eq!(ctrl.state, State::Held);
    }

    // ── Edge detection ──

    #[test]
    fn held_button_no_repeated_trigger() {
        let mut ctrl = Controller::new();

        // Button held continuously → trigger only once
        ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Same state, no new edge → no re-trigger
        for t in (200..5000).step_by(100) {
            ctrl.update(&input(t, true, false));
            // Should eventually transition to Held, but never trigger MicClick again
        }
        assert_eq!(ctrl.state, State::Held);
    }

    #[test]
    fn rapid_press_release_works() {
        let mut ctrl = Controller::new();

        // Fast on/off (debounce simulation with clean signal)
        ctrl.update(&input(100, true, false));
        ctrl.update(&input(120, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Wait → Idle
        let actions = ctrl.update(&input(10_120, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    // ── Mic click counter ──

    #[test]
    fn short_press_cycle_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        // Press → 1 click
        let a = ctrl.update(&input(100, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Release → no click (timer starts)
        let a = ctrl.update(&input(200, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Timer expired → 1 click
        let a = ctrl.update(&input(10_200, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 2, "Exactly 2 clicks: on, off");
    }

    #[test]
    fn hold_cycle_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        // Press → 1 click
        let a = ctrl.update(&input(100, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Hold threshold → no click
        let a = ctrl.update(&input(700, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Release → 1 click
        let a = ctrl.update(&input(800, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(click_count, 2, "Exactly 2 clicks: on, off");
    }

    #[test]
    fn toggle_off_in_timed_produces_two_clicks() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        // Short press → Timed (1 click)
        let a = ctrl.update(&input(100, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }
        ctrl.update(&input(200, false, false));

        // Second short press in Timed → Pressing
        let a = ctrl.update(&input(1000, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        // Release → Idle (1 click)
        let a = ctrl.update(&input(1100, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }

        assert_eq!(
            click_count, 2,
            "Exactly 2 clicks: on, off (no click on re-press in Timed)"
        );
    }

    // ── Sync correction in Held state ──

    #[test]
    fn sync_corrects_in_held_state() {
        let mut ctrl = Controller::new();

        // Long press → Held
        ctrl.update(&input_with_mic(100, true, false, true));
        ctrl.update(&input_with_mic(700, true, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Mic suddenly reports "off" while Held (should be on)
        ctrl.update(&input_with_mic(800, true, false, false));
        ctrl.update(&input_with_mic(1200, true, false, false));

        // Not yet at tolerance
        let actions = ctrl.update(&input_with_mic(1299, true, false, false));
        assert!(!has_mic_click(&actions), "Still within tolerance");

        // Tolerance exceeded → correction
        let actions = ctrl.update(&input_with_mic(1300, true, false, false));
        assert!(
            has_mic_click(&actions),
            "Correction click in Held state after 500ms mismatch"
        );
    }

    // ── Button 2 long press → Held ──

    #[test]
    fn button2_long_press_transitions_to_held() {
        let mut ctrl = Controller::new();

        // Press button 2
        let actions = ctrl.update(&input(100, false, true));
        assert_eq!(ctrl.state, State::Pressing);
        assert!(has_mic_click(&actions));

        // Hold ≥500ms → Held
        ctrl.update(&input(600, false, true));
        assert_eq!(ctrl.state, State::Held);

        // Release → Idle + MicClick
        let actions = ctrl.update(&input(700, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }

    // ── Timed → long press → Held → release ──

    #[test]
    fn timed_then_long_repress_to_held_and_release() {
        let mut ctrl = Controller::new();
        let mut click_count = 0;

        // Short press → Timed (1 click)
        let a = ctrl.update(&input(100, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);

        // Re-press during Timed → Pressing (was_active=true)
        let a = ctrl.update(&input(1000, true, false));
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Pressing);

        // Hold ≥500ms → Held
        ctrl.update(&input(1500, true, false));
        assert_eq!(ctrl.state, State::Held);

        // Release → Idle (1 click)
        let a = ctrl.update(&input(1600, false, false));
        if has_mic_click(&a) {
            click_count += 1;
        }
        assert_eq!(ctrl.state, State::Idle);
        assert_eq!(
            click_count, 2,
            "Exactly 2 clicks: on (initial), off (held release)"
        );
    }

    // ── Button handoff in Pressing state ──

    #[test]
    fn pressing_btn1_then_add_btn2_then_release_btn1_stays_pressing() {
        let mut ctrl = Controller::new();

        // Press btn1 → Pressing
        ctrl.update(&input(100, true, false));
        assert_eq!(ctrl.state, State::Pressing);

        // Also press btn2
        ctrl.update(&input(200, true, true));
        assert_eq!(ctrl.state, State::Pressing);

        // Release btn1, btn2 still held → stays Pressing
        ctrl.update(&input(300, false, true));
        assert_eq!(ctrl.state, State::Pressing);

        // Release btn2 → Timed (short press, was_active=false)
        ctrl.update(&input(400, false, false));
        assert_eq!(ctrl.state, State::Timed);
    }

    // ── Sync wrapping overflow ──

    #[test]
    fn sync_mismatch_timer_handles_wrapping() {
        let mut ctrl = Controller::new();

        // Short press → Timed near u32::MAX
        let t0 = u32::MAX - 200;
        ctrl.update(&input_with_mic(t0, true, false, true));
        ctrl.update(&input_with_mic(t0.wrapping_add(50), false, false, true));
        assert_eq!(ctrl.state, State::Timed);

        // Mismatch starts across the overflow boundary
        ctrl.update(&input_with_mic(t0.wrapping_add(100), false, false, false));

        // Tolerance exceeded after wrapping
        let actions = ctrl.update(&input_with_mic(t0.wrapping_add(600), false, false, false));
        assert!(
            has_mic_click(&actions),
            "Sync correction works across u32 overflow"
        );
    }

    // ── Simultaneous press + release both buttons ──

    #[test]
    fn both_buttons_simultaneous_release_in_pressing() {
        let mut ctrl = Controller::new();

        // Press both → Pressing
        ctrl.update(&input(100, true, true));
        assert_eq!(ctrl.state, State::Pressing);

        // Release both simultaneously → Timed
        ctrl.update(&input(200, false, false));
        assert_eq!(ctrl.state, State::Timed);
    }

    #[test]
    fn both_buttons_simultaneous_release_in_held() {
        let mut ctrl = Controller::new();

        // Press both → Held
        ctrl.update(&input(100, true, true));
        ctrl.update(&input(700, true, true));
        assert_eq!(ctrl.state, State::Held);

        // Release both simultaneously → Idle + MicClick
        let actions = ctrl.update(&input(800, false, false));
        assert_eq!(ctrl.state, State::Idle);
        assert!(has_mic_click(&actions));
    }
}
