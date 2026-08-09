//! Domain types for a coding session.
//!
//! Rust note: ava's Zig `Session` carried an `allocator` field and every
//! `Task` needed an explicit `deinit`. Here `String` and `Vec<Task>` own
//! their memory, and `Drop` frees it when a `Session` goes out of scope —
//! so there is nothing to free by hand and no allocator to thread through.

/// Tasks are short notes, capped so rendering never needs to wrap.
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
pub enum Pane {
    Active,
    Completed,
}

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
pub struct Session {
    /// Basename, not a full path. Reassigned when a session is resumed.
    pub filename: String,
    pub timestamp: Option<Timestamp>,
    pub active: Vec<Task>,
    pub completed: Vec<Task>,
    /// True when in-memory state diverges from what is on disk.
    pub dirty: bool,
}

/// Longest prefix of `s` that fits in `max` bytes without splitting a
/// character.
///
/// Rust note: ava sliced at exactly 150 bytes, which can cut a multi-byte
/// UTF-8 character in half and print mojibake. Rust's `&str` is guaranteed
/// valid UTF-8, so slicing mid-character would panic instead — the type
/// system forces us to get this right.
#[allow(dead_code)]
pub fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[allow(dead_code)]
impl Session {
    pub fn tasks(&self, pane: Pane) -> &[Task] {
        match pane {
            Pane::Active => &self.active,
            Pane::Completed => &self.completed,
        }
    }

    pub fn tasks_mut(&mut self, pane: Pane) -> &mut Vec<Task> {
        match pane {
            Pane::Active => &mut self.active,
            Pane::Completed => &mut self.completed,
        }
    }

    /// Append a task to `pane`. Empty text is ignored; long text is capped.
    pub fn add(&mut self, pane: Pane, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = truncate_on_char_boundary(text, MAX_TASK_BYTES).to_string();
        let done = pane == Pane::Completed;
        self.tasks_mut(pane).push(Task { text, done });
        self.dirty = true;
    }

    /// Remove the task at `index`. Out of range is a no-op.
    pub fn delete(&mut self, pane: Pane, index: usize) {
        let list = self.tasks_mut(pane);
        if index >= list.len() {
            return;
        }
        list.remove(index);
        self.dirty = true;
    }

    /// Move the task at `index` to the end of the other pane, flipping
    /// `done`.
    ///
    /// Rust note: ava's version returns an error and hand-rolls a rollback,
    /// because Zig's `append` can fail on allocation. `Vec::push` aborts on
    /// OOM rather than returning, so that entire failure branch disappears —
    /// this function cannot fail.
    pub fn toggle(&mut self, from: Pane, index: usize) {
        let src = self.tasks_mut(from);
        if index >= src.len() {
            return;
        }
        let mut task = src.remove(index);
        task.done = !task.done;
        self.tasks_mut(from.other()).push(task);
        self.dirty = true;
    }

    /// Swap two slots in the same pane. No-op if either index is out of
    /// range or they are equal.
    pub fn swap(&mut self, pane: Pane, a: usize, b: usize) {
        if a == b {
            return;
        }
        let list = self.tasks_mut(pane);
        if a >= list.len() || b >= list.len() {
            return;
        }
        list.swap(a, b);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_session() -> Session {
        Session {
            filename: "x.md".to_string(),
            timestamp: None,
            active: Vec::new(),
            completed: Vec::new(),
            dirty: false,
        }
    }

    #[test]
    fn add_then_delete_restores_count() {
        let mut s = empty_session();
        s.add(Pane::Active, "first");
        s.add(Pane::Active, "second");
        assert_eq!(s.active.len(), 2);
        assert!(s.dirty);

        s.delete(Pane::Active, 0);
        assert_eq!(s.active.len(), 1);
        assert_eq!(s.active[0].text, "second");
    }

    #[test]
    fn add_ignores_empty_text() {
        let mut s = empty_session();
        s.add(Pane::Active, "");
        assert!(s.active.is_empty());
        assert!(!s.dirty);
    }

    #[test]
    fn toggle_moves_task_between_panes_and_flips_done() {
        let mut s = empty_session();
        s.add(Pane::Active, "thing");
        s.dirty = false;

        s.toggle(Pane::Active, 0);
        assert!(s.active.is_empty());
        assert_eq!(s.completed.len(), 1);
        assert_eq!(s.completed[0].text, "thing");
        assert!(s.completed[0].done);
        assert!(s.dirty);

        s.toggle(Pane::Completed, 0);
        assert_eq!(s.active.len(), 1);
        assert!(s.completed.is_empty());
        assert!(!s.active[0].done);
    }

    #[test]
    fn toggle_out_of_range_is_a_noop() {
        let mut s = empty_session();
        s.toggle(Pane::Active, 5);
        assert!(s.active.is_empty());
        assert!(!s.dirty);
    }

    #[test]
    fn add_caps_text_at_max_bytes() {
        let mut s = empty_session();
        s.add(Pane::Active, &"x".repeat(200));
        assert_eq!(s.active[0].text.len(), MAX_TASK_BYTES);
    }

    #[test]
    fn add_truncates_on_char_boundary_not_mid_sequence() {
        let mut s = empty_session();
        // 149 ASCII bytes then a 2-byte char: cutting at 150 would split it.
        let text = format!("{}é", "x".repeat(MAX_TASK_BYTES - 1));
        s.add(Pane::Active, &text);
        assert_eq!(s.active[0].text.len(), MAX_TASK_BYTES - 1);
        assert!(s.active[0].text.is_char_boundary(s.active[0].text.len()));
    }

    #[test]
    fn swap_exchanges_two_slots() {
        let mut s = empty_session();
        s.add(Pane::Active, "first");
        s.add(Pane::Active, "second");
        s.swap(Pane::Active, 0, 1);
        assert_eq!(s.active[0].text, "second");
        assert_eq!(s.active[1].text, "first");
    }

    #[test]
    fn swap_out_of_range_or_equal_is_a_noop() {
        let mut s = empty_session();
        s.add(Pane::Active, "only");
        s.dirty = false;
        s.swap(Pane::Active, 0, 0);
        s.swap(Pane::Active, 0, 9);
        assert!(!s.dirty);
    }

    #[test]
    fn pane_other_flips() {
        assert_eq!(Pane::Active.other(), Pane::Completed);
        assert_eq!(Pane::Completed.other(), Pane::Active);
    }
}
