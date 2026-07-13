//! Agent lifecycle + PTY orchestration.
//!
//! Populated in M4 (PTY spawn + alacritty_terminal grid) and M5 (agent
//! registry + Captain routing + SQLite session persistence). M0 leaves
//! this module empty so `cargo check` is green.
