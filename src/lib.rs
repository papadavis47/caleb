//! caleb — track tasks for a coding session.
//!
//! Named for Caleb Smith in *Ex Machina*.
//!
//! The binary in `main.rs` is a thin CLI wrapper; everything testable lives
//! here. Layering runs one way: `model` holds the plain data types, `markdown`
//! and `storage` are pure leaves above it, `session` composes them into load
//! and save, and `ui`/`app`/`picker`/`tui` sit at the terminal edge.

pub mod app;
pub mod clean;
pub mod markdown;
pub mod model;
pub mod picker;
pub mod session;
pub mod storage;
#[cfg(test)]
pub(crate) mod test_util;
pub mod tui;
pub mod ui;
