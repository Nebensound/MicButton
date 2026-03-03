//! Mic Button Controller – Hardware-independent logic
//!
//! This library crate contains the testable logic.
//! Tests run on the host via `cargo test --lib`.

#![cfg_attr(not(test), no_std)]

pub mod logic;
