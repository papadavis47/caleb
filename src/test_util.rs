//! Helpers shared across the modules' unit tests.
//!
//! Rust note: this module is `#[cfg(test)]` in `lib.rs`, so it never reaches
//! the shipped binary. Integration tests under `tests/` compile the library
//! *without* `cfg(test)` and therefore cannot see it — they build their own
//! fixtures through the public API, which is the point of that layer.

use crate::model::{Pane, Task};
use crate::session::Session;

/// A task with the given text and done flag.
pub fn task(text: &str, done: bool) -> Task {
    Task {
        text: text.to_string(),
        done,
    }
}

/// A clean session with no tasks and no timestamp, named `x.md`.
pub fn empty_session() -> Session {
    Session {
        filename: "x.md".to_string(),
        timestamp: None,
        active: Vec::new(),
        completed: Vec::new(),
        dirty: false,
    }
}

/// A session pre-filled with active tasks, left non-dirty so tests can assert
/// on dirtiness caused by the operation under test.
pub fn session_with(active: &[&str]) -> Session {
    let mut session = empty_session();
    for t in active {
        session.add(Pane::Active, t);
    }
    session.dirty = false;
    session
}
