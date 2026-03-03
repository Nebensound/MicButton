# Copilot Instructions – MicButton

## Language

All code, comments, documentation, and commit messages must be in **English**.

## Project

ATtiny45 Mic Button Controller in Rust (`no_std`, AVR target).

## Architecture

- `src/main.rs` – Hardware-specific (GPIO, Timer0, ISR). Only compiles for AVR.
- `src/logic.rs` – Hardware-independent state machine. Testable on the host.
- `src/lib.rs` – Library crate, exports `logic` for tests.

## Build Commands

- **AVR build:** `cargo avr-build` (alias for `cargo build --release -Z build-std=core --target avr-attiny45.json`)
- **Tests:** `cargo test --lib`
- **Flash:** `make flash` (build + objcopy + avrdude)
- **Alternative:** `cargo avr-flash` (uses runner from `.cargo/config.toml`)

## Key Conventions

- As much logic as possible must be hardware-independent in `logic.rs` so it can be tested locally on the host.
- Every new function must include corresponding tests in `logic.rs`.
- Dependencies (`avr-device`, `panic-halt`) are gated on `cfg(target_arch = "avr")` in `Cargo.toml`.
- `build-std` is passed via CLI, **not** in `.cargo/config.toml` (otherwise breaks host tests).
- New logic belongs in `logic.rs` with tests – not in `main.rs`.
- `unsafe` only for `interrupt::enable()` with a `// SAFETY:` comment.
- Controller returns `Action` arrays, `main.rs` applies them to hardware.

## Pin Assignment

See [README.md](../README.md#attiny45-pin-assignment).

## State Machine

`Idle` → `Pressing` → `Timed` (10 s) or `Held` → `Idle`

Both buttons are interchangeable. Mic sync corrects after 500 ms mismatch.
