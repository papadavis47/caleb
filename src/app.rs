//! The event loop's state and key dispatch.
//!
//! `App` owns view state and mutates the `Session`. It never touches the
//! terminal — `main` and `tui` do that — which is what makes every binding
//! testable without a pty.

use crate::session::{MAX_TASK_BYTES, Pane, SaveError, Session};
use crate::ui::{self, Overlay, Palette, PaneRects, ViewState};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Mode {
    Normal,
    AddInput,
    Help,
}

#[allow(dead_code)]
pub struct App {
    pub session: Session,
    pub storage_dir: PathBuf,
    pub palette: Palette,
    pub focused: Pane,
    pub active_cursor: usize,
    pub completed_cursor: usize,
    pub active_scroll: usize,
    pub completed_scroll: usize,
    pub mode: Mode,
    pub input: String,
    pub quit: bool,
    /// Pane geometry from the last frame, for mouse hit-testing.
    pub pane_rects: PaneRects,
    pub last_click: Option<(Instant, Pane, usize)>,
}

#[allow(dead_code)]
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
            last_click: None,
        }
    }

    pub fn view_state(&self) -> ViewState<'_> {
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

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<(), SaveError> {
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
            KeyCode::Char(' ') | KeyCode::Char('x') => {
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
                // Rust note: `pop` removes a whole `char`, so multi-byte
                // characters come off in one press. ava had to walk back
                // over UTF-8 continuation bytes by hand.
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

    pub fn clamp_cursors(&mut self) {
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
    pub fn adjust_scroll(&mut self, pane_height: u16) {
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

    fn app_with(tasks: &[&str]) -> App {
        let mut session = Session {
            filename: "x.md".to_string(),
            timestamp: None,
            active: Vec::new(),
            completed: Vec::new(),
            dirty: false,
        };
        for t in tasks {
            session.add(Pane::Active, t);
        }
        session.dirty = false;
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
        assert_eq!(app.input.len(), crate::session::MAX_TASK_BYTES);
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
        app.active_scroll = 99;
        app.active_cursor = 0;
        app.adjust_scroll(8);
        assert_eq!(app.active_scroll, 0);
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
}
