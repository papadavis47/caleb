//! The event loop's state and key dispatch.
//!
//! `App` owns view state and mutates the `Session`. It never touches the
//! terminal — `main` and `tui` do that — which is what makes every binding
//! testable without a pty.

use crate::model::{MAX_TASK_BYTES, Pane};
use crate::session::{SaveError, Session};
use crate::tui::Tui;
use crate::ui::{self, ClickTracker, Overlay, Palette, PaneRects, ViewState};
use crossterm::event::{self, Event, KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use std::path::PathBuf;
use std::time::Instant;
use thiserror::Error;

/// Anything that can end the event loop early.
///
/// Draw and read failures are `io`; the exit-time autosave is its own variant
/// so the message never blames the wrong subsystem.
#[derive(Debug, Error)]
pub enum RunError {
    #[error("terminal I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Save(#[from] SaveError),
}

/// What a screen coordinate resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Task { pane: Pane, index: usize },
    Pane(Pane),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    AddInput,
    Help,
}

#[derive(Debug)]
pub struct App {
    session: Session,
    storage_dir: PathBuf,
    palette: Palette,
    focused: Pane,
    active_cursor: usize,
    completed_cursor: usize,
    active_scroll: usize,
    completed_scroll: usize,
    mode: Mode,
    input: String,
    quit: bool,
    /// Pane geometry from the last frame, for mouse hit-testing.
    pane_rects: PaneRects,
    last_click: ClickTracker<(Pane, usize)>,
}

impl App {
    pub fn new(session: Session, storage_dir: PathBuf, palette: Palette) -> Self {
        Self {
            session,
            storage_dir,
            palette,
            focused: Pane::Active,
            active_cursor: 0,
            completed_cursor: 0,
            active_scroll: 0,
            completed_scroll: 0,
            mode: Mode::Normal,
            input: String::new(),
            quit: false,
            pane_rects: PaneRects::default(),
            last_click: ClickTracker::default(),
        }
    }

    fn view_state(&self) -> ViewState<'_> {
        ViewState {
            timestamp: self.session.timestamp,
            active: &self.session.active,
            completed: &self.session.completed,
            focused: self.focused,
            active_cursor: self.active_cursor,
            completed_cursor: self.completed_cursor,
            active_scroll: self.active_scroll,
            completed_scroll: self.completed_scroll,
            dirty: self.session.dirty,
            palette: self.palette,
            overlay: match self.mode {
                Mode::Normal => Overlay::None,
                Mode::AddInput => Overlay::Input(&self.input),
                Mode::Help => Overlay::Help,
            },
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<(), SaveError> {
        match self.mode {
            Mode::Help => {
                // Any real key dismisses; it does not also act.
                if !matches!(key.code, KeyCode::Null) {
                    self.mode = Mode::Normal;
                }
                return Ok(());
            }
            Mode::AddInput => {
                self.handle_input_key(key);
                return Ok(());
            }
            Mode::Normal => {}
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focused = Pane::Active,
            KeyCode::Char('l') | KeyCode::Right => self.focused = Pane::Completed,
            KeyCode::Char('g') => *self.cursor_mut() = 0,
            KeyCode::Char('G') => {
                let len = self.session.tasks(self.focused).len();
                *self.cursor_mut() = len.saturating_sub(1);
            }
            // `Session::toggle`/`delete` no-op on an out-of-range index, so
            // there is no bounds check to duplicate here.
            KeyCode::Char(' ' | 'x') => {
                let (pane, cursor) = (self.focused, *self.cursor_mut());
                self.session.toggle(pane, cursor);
            }
            KeyCode::Char('d') => {
                let (pane, cursor) = (self.focused, *self.cursor_mut());
                self.session.delete(pane, cursor);
            }
            KeyCode::Char('s') => {
                let dir = self.storage_dir.clone();
                self.session.save(&dir)?;
            }
            KeyCode::Char('a') => {
                self.input.clear();
                self.mode = Mode::AddInput;
            }
            KeyCode::Char('J') => self.swap_cursor(1),
            KeyCode::Char('K') => self.swap_cursor(-1),
            KeyCode::Char('?') => self.mode = Mode::Help,
            _ => {}
        }

        self.clamp_cursors();
        Ok(())
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.input.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    let text = std::mem::take(&mut self.input);
                    self.session.add(Pane::Active, &text);
                }
                self.input.clear();
                self.mode = Mode::Normal;
                self.clamp_cursors();
            }
            KeyCode::Backspace => {
                // Rust note: `String::pop` removes a whole `char`, so a
                // multi-byte character comes off in one press — no walking
                // back over UTF-8 continuation bytes by hand.
                self.input.pop();
            }
            // Control characters never reach the buffer, and the cap
            // matches what `Session::add` would keep anyway.
            KeyCode::Char(c)
                if !c.is_control() && self.input.len() + c.len_utf8() <= MAX_TASK_BYTES =>
            {
                self.input.push(c);
            }
            _ => {}
        }
    }

    fn cursor_mut(&mut self) -> &mut usize {
        match self.focused {
            Pane::Active => &mut self.active_cursor,
            Pane::Completed => &mut self.completed_cursor,
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.session.tasks(self.focused).len();
        let cursor = self.cursor_mut();
        if len == 0 {
            *cursor = 0;
            return;
        }
        if delta > 0 && *cursor + 1 < len {
            *cursor += 1;
        } else if delta < 0 && *cursor > 0 {
            *cursor -= 1;
        }
    }

    fn swap_cursor(&mut self, delta: isize) {
        let pane = self.focused;
        let len = self.session.tasks(pane).len();
        let cursor = *self.cursor_mut();
        let target = if delta > 0 {
            if cursor + 1 >= len {
                return;
            }
            cursor + 1
        } else {
            if cursor == 0 {
                return;
            }
            cursor - 1
        };
        self.session.swap(pane, cursor, target);
        *self.cursor_mut() = target;
    }

    fn clamp_cursors(&mut self) {
        let a = self.session.active.len();
        self.active_cursor = if a == 0 {
            0
        } else {
            self.active_cursor.min(a - 1)
        };
        let c = self.session.completed.len();
        self.completed_cursor = if c == 0 {
            0
        } else {
            self.completed_cursor.min(c - 1)
        };
    }

    /// Keep both panes' scroll offsets consistent with their cursors.
    /// `pane_height` is the full pane height including borders.
    fn adjust_scroll(&mut self, pane_height: u16) {
        let visible = ui::visible_tasks(pane_height);
        clamp_pane(
            &mut self.active_scroll,
            self.active_cursor,
            self.session.active.len(),
            visible,
        );
        clamp_pane(
            &mut self.completed_scroll,
            self.completed_cursor,
            self.session.completed.len(),
            visible,
        );
    }

    /// Resolve a screen coordinate against the last frame's pane geometry.
    ///
    /// The 2-row stride makes the arithmetic fall out for free: integer
    /// division maps a spacer row and the content row beneath it to the same
    /// index, so clicking either selects the task they belong to.
    fn hit_test(&self, col: u16, row: u16) -> Option<Hit> {
        let (pane, rect) = if contains(self.pane_rects.active, col, row) {
            (Pane::Active, self.pane_rects.active)
        } else if contains(self.pane_rects.completed, col, row) {
            (Pane::Completed, self.pane_rects.completed)
        } else {
            return None;
        };

        // Inside the borders?
        let top = rect.y + 1;
        let bottom = rect.y + rect.height - 1;
        if row < top || row >= bottom || col == rect.x || col == rect.x + rect.width - 1 {
            return Some(Hit::Pane(pane));
        }

        let scroll = match pane {
            Pane::Active => self.active_scroll,
            Pane::Completed => self.completed_scroll,
        };
        let row_offset = (row - top) as usize;
        let index = scroll + row_offset / ui::ROW_STRIDE as usize;

        if index < self.session.tasks(pane).len() {
            Some(Hit::Task { pane, index })
        } else {
            Some(Hit::Pane(pane))
        }
    }

    /// Dispatch a mouse event. `now` is injected so double-click timing is
    /// testable without sleeping.
    fn handle_mouse(&mut self, event: MouseEvent, now: Instant) {
        // The add-task field owns the screen while it is open.
        if self.mode == Mode::AddInput {
            return;
        }
        if self.mode == Mode::Help {
            if matches!(event.kind, MouseEventKind::Down(_)) {
                self.mode = Mode::Normal;
            }
            return;
        }

        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_focused(-1),
            MouseEventKind::ScrollDown => self.scroll_focused(1),
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_click(event.column, event.row, now);
            }
            _ => {}
        }
    }

    fn handle_click(&mut self, col: u16, row: u16, now: Instant) {
        let Some(hit) = self.hit_test(col, row) else {
            return;
        };

        match hit {
            Hit::Pane(pane) => {
                self.focused = pane;
                self.last_click.reset();
            }
            Hit::Task { pane, index } => {
                let is_double = self.last_click.click((pane, index), now);

                self.focused = pane;
                match pane {
                    Pane::Active => self.active_cursor = index,
                    Pane::Completed => self.completed_cursor = index,
                }

                if is_double {
                    self.session.toggle(pane, index);
                    self.clamp_cursors();
                }
            }
        }
    }

    fn scroll_focused(&mut self, delta: isize) {
        let scroll = match self.focused {
            Pane::Active => &mut self.active_scroll,
            Pane::Completed => &mut self.completed_scroll,
        };
        if delta > 0 {
            *scroll += 1; // bounded by clamp_scroll_only on the next frame
        } else if *scroll > 0 {
            *scroll -= 1;
        }
    }

    /// Bound scroll offsets without dragging them back to the cursor — used
    /// after a wheel event, where the cursor deliberately stays put.
    fn clamp_scroll_only(&mut self, pane_height: u16) {
        let visible = ui::visible_tasks(pane_height);
        let max = |len: usize| {
            if visible == 0 || len <= visible {
                0
            } else {
                len - visible
            }
        };
        self.active_scroll = self.active_scroll.min(max(self.session.active.len()));
        self.completed_scroll = self.completed_scroll.min(max(self.session.completed.len()));
    }

    // --- event loop ---

    /// Apply one input event against a frame of `pane_height` rows.
    ///
    /// Split out of [`App::run`] so the dispatch rules — press-only filtering,
    /// and which scroll clamp pairs with which event — are testable without a
    /// pty.
    fn handle_event(
        &mut self,
        event: &Event,
        now: Instant,
        pane_height: u16,
    ) -> Result<(), SaveError> {
        match *event {
            Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                // Press-only: terminals that also report releases would
                // otherwise run every binding twice.
                self.handle_key(key)?;
                self.adjust_scroll(pane_height);
            }
            Event::Mouse(m) => {
                self.handle_mouse(m, now);
                // Wheel scrolling leaves the cursor alone, so the offset is
                // bounded rather than dragged back to it.
                self.clamp_scroll_only(pane_height);
            }
            // Resize needs no work — the next draw reads the new size.
            _ => {}
        }
        Ok(())
    }

    /// Draw, wait for an event, dispatch, repeat. Auto-saves on the way out.
    pub fn run(&mut self, tui: &mut Tui) -> Result<(), RunError> {
        while !self.quit {
            let mut rects = self.pane_rects;
            tui.terminal().draw(|frame| {
                rects = ui::draw(frame, &self.view_state());
            })?;
            self.pane_rects = rects;

            // Pane geometry from the frame just drawn — the same rows the
            // user is looking at when the next event arrives.
            let pane_height = self.pane_rects.active.height;

            self.handle_event(&event::read()?, Instant::now(), pane_height)?;
        }

        if self.session.dirty {
            let dir = self.storage_dir.clone();
            self.session.save(&dir)?;
        }
        Ok(())
    }
}

/// Whether a screen coordinate falls inside a pane's rendered rectangle.
fn contains(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn clamp_pane(scroll: &mut usize, cursor: usize, len: usize, visible: usize) {
    // Keep the cursor visible.
    if cursor < *scroll {
        *scroll = cursor;
    }
    if visible > 0 && cursor >= *scroll + visible {
        *scroll = cursor - visible + 1;
    }
    // Never scroll past the end.
    let max_scroll = if visible == 0 || len <= visible {
        0
    } else {
        len - visible
    };
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use std::time::Duration;

    fn app_with(tasks: &[&str]) -> App {
        let session = crate::test_util::session_with(tasks);
        App::new(session, PathBuf::from("/nonexistent"), Palette::new(false))
    }

    fn press(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .unwrap();
    }

    #[test]
    fn q_sets_quit() {
        let mut app = app_with(&[]);
        press(&mut app, 'q');
        assert!(app.quit);
    }

    #[test]
    fn j_moves_down_within_bounds() {
        let mut app = app_with(&["a", "b"]);
        press(&mut app, 'j');
        assert_eq!(app.active_cursor, 1);
        press(&mut app, 'j');
        assert_eq!(app.active_cursor, 1);
    }

    #[test]
    fn k_moves_up_and_stops_at_zero() {
        let mut app = app_with(&["a", "b"]);
        press(&mut app, 'j');
        press(&mut app, 'k');
        assert_eq!(app.active_cursor, 0);
        press(&mut app, 'k');
        assert_eq!(app.active_cursor, 0);
    }

    #[test]
    fn h_and_l_switch_panes() {
        let mut app = app_with(&[]);
        press(&mut app, 'l');
        assert_eq!(app.focused, Pane::Completed);
        press(&mut app, 'h');
        assert_eq!(app.focused, Pane::Active);
    }

    #[test]
    fn g_and_shift_g_jump_to_ends() {
        let mut app = app_with(&["a", "b", "c"]);
        press(&mut app, 'G');
        assert_eq!(app.active_cursor, 2);
        press(&mut app, 'g');
        assert_eq!(app.active_cursor, 0);
    }

    #[test]
    fn space_toggles_across_panes() {
        let mut app = app_with(&["thing"]);
        press(&mut app, ' ');
        assert!(app.session.active.is_empty());
        assert_eq!(app.session.completed.len(), 1);
        assert_eq!(app.active_cursor, 0);
    }

    #[test]
    fn d_deletes_the_selected_task() {
        let mut app = app_with(&["a", "b"]);
        press(&mut app, 'd');
        assert_eq!(app.session.active.len(), 1);
        assert_eq!(app.session.active[0].text, "b");
    }

    #[test]
    fn a_enters_add_mode() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        assert_eq!(app.mode, Mode::AddInput);
        assert!(app.input.is_empty());
    }

    #[test]
    fn typing_then_enter_creates_a_task() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        for c in "hello world".chars() {
            press(&mut app, c);
        }
        assert_eq!(app.input, "hello world");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.session.active[0].text, "hello world");
    }

    #[test]
    fn enter_on_empty_input_adds_nothing() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(app.session.active.is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn esc_cancels_add_mode_and_discards() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        press(&mut app, 'x');
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty());
        assert!(app.session.active.is_empty());
    }

    #[test]
    fn backspace_pops_a_whole_utf8_character() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        for c in "abé".chars() {
            press(&mut app, c);
        }
        assert_eq!(app.input.len(), 4); // é is 2 bytes
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input, "ab");
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input, "a");
    }

    #[test]
    fn input_stops_at_the_byte_cap() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        for _ in 0..200 {
            press(&mut app, 'x');
        }
        assert_eq!(app.input.len(), crate::model::MAX_TASK_BYTES);
    }

    #[test]
    fn multibyte_char_is_rejected_whole_at_the_byte_cap() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        for _ in 0..149 {
            press(&mut app, 'x');
        }
        assert_eq!(app.input.len(), 149);
        // 'é' is 2 bytes: 149 + 2 = 151 > 150, so it must be rejected whole —
        // never admitted as a single truncated byte.
        press(&mut app, 'é');
        assert_eq!(app.input.len(), 149);
        assert!(!app.input.ends_with('é'));
    }

    #[test]
    fn multibyte_char_fits_exactly_at_the_byte_cap() {
        let mut app = app_with(&[]);
        press(&mut app, 'a');
        for _ in 0..148 {
            press(&mut app, 'x');
        }
        // 148 + 2 = 150 == MAX_TASK_BYTES, so it must be accepted.
        press(&mut app, 'é');
        assert_eq!(app.input.len(), crate::model::MAX_TASK_BYTES);
        assert!(app.input.ends_with('é'));
    }

    #[test]
    fn shift_j_swaps_with_the_next_task() {
        let mut app = app_with(&["first", "second"]);
        press(&mut app, 'J');
        assert_eq!(app.session.active[0].text, "second");
        assert_eq!(app.session.active[1].text, "first");
        assert_eq!(app.active_cursor, 1);
    }

    #[test]
    fn shift_k_swaps_with_the_previous_task() {
        let mut app = app_with(&["first", "second"]);
        press(&mut app, 'j');
        press(&mut app, 'K');
        assert_eq!(app.session.active[0].text, "second");
        assert_eq!(app.active_cursor, 0);
    }

    #[test]
    fn question_mark_opens_help_and_any_key_dismisses() {
        let mut app = app_with(&[]);
        press(&mut app, '?');
        assert_eq!(app.mode, Mode::Help);
        press(&mut app, 'j');
        assert_eq!(app.mode, Mode::Normal);
        // The dismissing key must not also move the cursor.
        assert_eq!(app.active_cursor, 0);
    }

    #[test]
    fn arrow_keys_mirror_hjkl() {
        let mut app = app_with(&["a", "b"]);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.active_cursor, 1);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focused, Pane::Completed);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.focused, Pane::Active);
    }

    #[test]
    fn scroll_clamp_keeps_the_cursor_visible() {
        let mut app = app_with(&["a", "b", "c", "d", "e", "f"]);
        app.active_cursor = 5;
        // Pane 8 rows tall -> 6 inner rows -> 3 visible tasks.
        app.adjust_scroll(8);
        assert_eq!(app.active_scroll, 3);
    }

    #[test]
    fn scroll_clamp_never_passes_the_end() {
        let mut app = app_with(&["a", "b", "c", "d"]);
        app.active_cursor = 2;
        app.active_scroll = 99;
        // Pane 8 rows tall -> 6 inner rows -> 3 visible tasks, so the
        // furthest valid offset is len - visible = 1. A wrong max_scroll
        // formula leaves the cursor-visibility clamp's value of 2 in place
        // and fails here.
        app.adjust_scroll(8);
        assert_eq!(app.active_scroll, 1);
    }

    #[test]
    fn clamp_cursors_pulls_back_past_the_end() {
        let mut app = app_with(&["a", "b"]);
        app.active_cursor = 5;
        app.clamp_cursors();
        assert_eq!(app.active_cursor, 1);
        app.session.active.clear();
        app.clamp_cursors();
        assert_eq!(app.active_cursor, 0);
    }

    fn app_with_layout(active: &[&str], completed: &[&str]) -> App {
        let mut app = app_with(active);
        for t in completed {
            app.session.add(Pane::Completed, t);
        }
        app.session.dirty = false;
        // Mirrors a 40x10 frame: panes occupy rows 1..=8, 20 cols each.
        app.pane_rects = PaneRects {
            active: Rect {
                x: 0,
                y: 1,
                width: 20,
                height: 8,
            },
            completed: Rect {
                x: 20,
                y: 1,
                width: 20,
                height: 8,
            },
        };
        app
    }

    #[test]
    fn handle_event_ignores_key_releases() {
        // Terminals that report releases would otherwise run every binding
        // twice — 'd' would delete two tasks per keypress.
        let mut app = app_with(&["a", "b"]);
        let mut release = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        release.kind = event::KeyEventKind::Release;

        app.handle_event(&Event::Key(release), Instant::now(), 8)
            .unwrap();
        assert_eq!(app.session.active.len(), 2, "release must not delete");

        let mut down = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        down.kind = event::KeyEventKind::Press;
        app.handle_event(&Event::Key(down), Instant::now(), 8)
            .unwrap();
        assert_eq!(app.session.active.len(), 1, "press must delete");
    }

    #[test]
    fn handle_event_ignores_resize() {
        let mut app = app_with(&["a"]);
        app.handle_event(&Event::Resize(10, 10), Instant::now(), 8)
            .unwrap();
        assert_eq!(app.session.active.len(), 1);
        assert!(!app.quit);
    }

    #[test]
    fn handle_event_pairs_wheel_scroll_with_the_cursor_preserving_clamp() {
        // A wheel event must bound the offset without dragging it back to the
        // cursor; a key event does the opposite.
        let tasks: Vec<String> = (0..10).map(|i| format!("task{i}")).collect();
        let refs: Vec<&str> = tasks.iter().map(String::as_str).collect();
        let mut app = app_with(&refs);
        app.pane_rects = PaneRects {
            active: ratatui::layout::Rect::new(0, 3, 20, 8),
            completed: ratatui::layout::Rect::new(20, 3, 20, 8),
        };

        let wheel = click(MouseEventKind::ScrollDown, 5, 5);
        app.handle_event(&Event::Mouse(wheel), Instant::now(), 8)
            .unwrap();
        assert_eq!(app.active_scroll, 1);
        assert_eq!(app.active_cursor, 0, "wheel leaves the cursor put");

        // A key event re-syncs the offset to the cursor.
        let mut key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        key.kind = event::KeyEventKind::Press;
        app.handle_event(&Event::Key(key), Instant::now(), 8)
            .unwrap();
        assert_eq!(app.active_scroll, 0);
    }

    fn click(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn down(col: u16, row: u16) -> MouseEvent {
        click(MouseEventKind::Down(MouseButton::Left), col, row)
    }

    #[test]
    fn hit_test_finds_a_task_on_its_content_row() {
        let app = app_with_layout(&["a", "b"], &[]);
        // Pane top border row 1, inner rows 2..=7. Task 0 content is row 3.
        assert_eq!(
            app.hit_test(5, 3),
            Some(Hit::Task {
                pane: Pane::Active,
                index: 0
            })
        );
        assert_eq!(
            app.hit_test(5, 5),
            Some(Hit::Task {
                pane: Pane::Active,
                index: 1
            })
        );
    }

    #[test]
    fn hit_test_maps_a_spacer_row_to_its_task() {
        let app = app_with_layout(&["a", "b"], &[]);
        // Row 2 is the spacer above task 0; row 4 the spacer above task 1.
        assert_eq!(
            app.hit_test(5, 2),
            Some(Hit::Task {
                pane: Pane::Active,
                index: 0
            })
        );
        assert_eq!(
            app.hit_test(5, 4),
            Some(Hit::Task {
                pane: Pane::Active,
                index: 1
            })
        );
    }

    #[test]
    fn hit_test_past_the_end_is_a_pane_hit() {
        let app = app_with_layout(&["a"], &[]);
        assert_eq!(app.hit_test(5, 7), Some(Hit::Pane(Pane::Active)));
    }

    #[test]
    fn hit_test_resolves_the_right_pane() {
        let app = app_with_layout(&[], &["done"]);
        assert_eq!(
            app.hit_test(25, 3),
            Some(Hit::Task {
                pane: Pane::Completed,
                index: 0
            })
        );
    }

    #[test]
    fn hit_test_on_a_border_is_a_pane_hit() {
        let app = app_with_layout(&["a"], &[]);
        assert_eq!(app.hit_test(0, 1), Some(Hit::Pane(Pane::Active)));
    }

    #[test]
    fn hit_test_outside_every_pane_is_none() {
        let app = app_with_layout(&["a"], &[]);
        assert_eq!(app.hit_test(5, 0), None); // header
        assert_eq!(app.hit_test(5, 9), None); // status bar
    }

    #[test]
    fn hit_test_accounts_for_scroll() {
        let mut app = app_with_layout(&["a", "b", "c", "d", "e"], &[]);
        app.active_scroll = 2;
        assert_eq!(
            app.hit_test(5, 3),
            Some(Hit::Task {
                pane: Pane::Active,
                index: 2
            })
        );
    }

    #[test]
    fn click_selects_a_task_and_moves_focus() {
        let mut app = app_with_layout(&["a"], &["done"]);
        let t0 = Instant::now();
        app.handle_mouse(down(25, 3), t0);
        assert_eq!(app.focused, Pane::Completed);
        assert_eq!(app.completed_cursor, 0);
    }

    #[test]
    fn click_on_empty_pane_area_only_moves_focus() {
        let mut app = app_with_layout(&["a"], &[]);
        app.active_cursor = 0;
        let t0 = Instant::now();
        app.handle_mouse(down(25, 5), t0);
        assert_eq!(app.focused, Pane::Completed);
        assert_eq!(app.completed_cursor, 0);
    }

    #[test]
    fn double_click_toggles_the_task() {
        let mut app = app_with_layout(&["thing"], &[]);
        let t0 = Instant::now();
        app.handle_mouse(down(5, 3), t0);
        app.handle_mouse(down(5, 3), t0 + Duration::from_millis(200));
        assert!(app.session.active.is_empty());
        assert_eq!(app.session.completed.len(), 1);
    }

    #[test]
    fn slow_second_click_does_not_toggle() {
        let mut app = app_with_layout(&["thing"], &[]);
        let t0 = Instant::now();
        app.handle_mouse(down(5, 3), t0);
        app.handle_mouse(down(5, 3), t0 + Duration::from_millis(900));
        assert_eq!(app.session.active.len(), 1);
    }

    #[test]
    fn second_click_on_a_different_task_does_not_toggle() {
        let mut app = app_with_layout(&["a", "b"], &[]);
        let t0 = Instant::now();
        app.handle_mouse(down(5, 3), t0);
        app.handle_mouse(down(5, 5), t0 + Duration::from_millis(100));
        assert_eq!(app.session.active.len(), 2);
        assert_eq!(app.active_cursor, 1);
    }

    #[test]
    fn wheel_scrolls_the_focused_pane_without_moving_the_cursor() {
        let mut app = app_with_layout(&["a", "b", "c", "d", "e", "f"], &[]);
        let t0 = Instant::now();
        app.handle_mouse(click(MouseEventKind::ScrollDown, 5, 3), t0);
        assert_eq!(app.active_scroll, 1);
        assert_eq!(app.active_cursor, 0);
        app.handle_mouse(click(MouseEventKind::ScrollUp, 5, 3), t0);
        assert_eq!(app.active_scroll, 0);
    }

    #[test]
    fn mouse_is_ignored_in_add_mode() {
        let mut app = app_with_layout(&["a"], &[]);
        app.mode = Mode::AddInput;
        let t0 = Instant::now();
        app.handle_mouse(down(25, 3), t0);
        assert_eq!(app.focused, Pane::Active);
    }

    #[test]
    fn click_dismisses_the_help_overlay() {
        let mut app = app_with_layout(&["a"], &[]);
        app.mode = Mode::Help;
        let t0 = Instant::now();
        app.handle_mouse(down(5, 3), t0);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn mouse_up_does_not_trigger_selection() {
        let mut app = app_with_layout(&["a"], &["done"]);
        let t0 = Instant::now();
        app.handle_mouse(click(MouseEventKind::Up(MouseButton::Left), 25, 3), t0);
        assert_eq!(app.focused, Pane::Active);
    }
}
