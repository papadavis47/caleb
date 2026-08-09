//! Domain types for a coding session.
//!
//! Rust note: ava's Zig `Session` carried an `allocator` field and every
//! `Task` needed an explicit `deinit`. Here `String` and `Vec<Task>` own
//! their memory, and `Drop` frees it when a `Session` goes out of scope —
//! so there is nothing to free by hand and no allocator to thread through.

/// Tasks are short notes, capped so rendering never needs to wrap.
#[allow(dead_code)]
pub const MAX_TASK_BYTES: usize = 150;

/// A single task. `text` is owned outright — no lifetime, no manual free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub text: String,
    pub done: bool,
}

/// Wall-clock time at minute granularity — all we need for filenames and
/// the markdown header.
///
/// Rust note: `Option<Timestamp>` replaces Zig's `?Timestamp`. The compiler
/// forces every read to handle the `None` case, so a missing header cannot
/// silently become a zero date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Timestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// Which pane has focus. Movement and toggling apply to whichever this names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Pane {
    Active,
    Completed,
}

#[allow(dead_code)]
impl Pane {
    /// The opposite pane. Toggling moves a task from one to the other.
    pub fn other(self) -> Self {
        match self {
            Pane::Active => Pane::Completed,
            Pane::Completed => Pane::Active,
        }
    }
}

/// All in-memory state for an open session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Session {
    /// Basename, not a full path. Reassigned when a session is resumed.
    pub filename: String,
    pub timestamp: Option<Timestamp>,
    pub active: Vec<Task>,
    pub completed: Vec<Task>,
    /// True when in-memory state diverges from what is on disk.
    pub dirty: bool,
}
