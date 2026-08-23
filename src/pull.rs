//! `p` — pull open tasks out of a past session into the current one.
//!
//! Two stages: choose a session, then choose which of its open tasks come
//! across. Every transition lives in [`PullState::on_key`], which is pure, so
//! the whole flow is unit-tested without a pty — the same split that makes
//! `picker`'s helpers testable. [`run`] is only a draw/read/dispatch loop.

use crate::markdown;
use crate::picker::{self, Entry};
use crate::tui::Tui;
use crate::ui::Palette;
use crossterm::event::{self, Event, KeyCode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem};
use std::path::Path;

/// A past session with something worth pulling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    /// Each open task's index in the file's `active` list, and its text. The
    /// index is what [`crate::session::pull_from_file`] needs; the text is
    /// what the screen shows.
    pub open: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Sessions,
    Tasks,
}

/// A confirmed pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub source: String,
    /// Indices into the source's `active` list, ascending and unique.
    pub indices: Vec<usize>,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stay,
    Cancel,
    Pull(Pulled),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullState {
    candidates: Vec<Candidate>,
    stage: Stage,
    session_cursor: usize,
    /// Parallel to the chosen candidate's `open`. Empty in `Stage::Sessions`.
    selected: Vec<bool>,
    task_cursor: usize,
}

/// Past sessions that have at least one pullable task, newest first.
///
/// "Pullable" is narrower than the picker's `open` count and the difference is
/// load-bearing: `count_tasks` counts every `- [ ]` line wherever it sits,
/// while `parse` files tasks by the heading above them. A hand-written `- [ ]`
/// under `## Completed` is `open` but is not in `active`, and a pull moves
/// tasks out of `active`. Counting the wrong one would put a session on stage
/// one whose stage two is empty.
///
/// A file that fails to parse is dropped here rather than erroring in the
/// middle of the flow. `Entry` already carries the contents `scan` read, so
/// this costs no extra I/O.
pub fn candidates(entries: &[Entry], current: &str) -> Vec<Candidate> {
    entries
        .iter()
        .filter(|e| e.name != current)
        .filter_map(|e| {
            let parsed = markdown::parse(&e.contents).ok()?;
            let open: Vec<(usize, String)> = parsed
                .active
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.done)
                .map(|(i, t)| (i, t.text.clone()))
                .collect();
            if open.is_empty() {
                return None;
            }
            Some(Candidate {
                name: e.name.clone(),
                open,
            })
        })
        .collect()
}

impl PullState {
    pub fn new(entries: &[Entry], current: &str) -> Self {
        Self {
            candidates: candidates(entries, current),
            stage: Stage::Sessions,
            session_cursor: 0,
            selected: Vec::new(),
            task_cursor: 0,
        }
    }

    /// Whether there is anything to pull. The screen says so, and every key
    /// cancels.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// The session the cursor is on, if any.
    fn current(&self) -> Option<&Candidate> {
        self.candidates.get(self.session_cursor)
    }

    pub fn on_key(&mut self, code: KeyCode) -> Step {
        if self.is_empty() {
            return Step::Cancel;
        }
        match self.stage {
            Stage::Sessions => self.session_key(code),
            Stage::Tasks => self.task_key(code),
        }
    }

    fn session_key(&mut self, code: KeyCode) -> Step {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Step::Cancel,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.session_cursor + 1 < self.candidates.len() {
                    self.session_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.session_cursor = self.session_cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                let count = self.current().map_or(0, |c| c.open.len());
                self.selected = vec![true; count];
                self.task_cursor = 0;
                self.stage = Stage::Tasks;
            }
            _ => {}
        }
        Step::Stay
    }

    fn task_key(&mut self, code: KeyCode) -> Step {
        match code {
            KeyCode::Char('q') => return Step::Cancel,
            KeyCode::Esc => self.stage = Stage::Sessions,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.task_cursor + 1 < self.selected.len() {
                    self.task_cursor += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.task_cursor = self.task_cursor.saturating_sub(1);
            }
            KeyCode::Char(' ') => {
                if let Some(flag) = self.selected.get_mut(self.task_cursor) {
                    *flag = !*flag;
                }
            }
            KeyCode::Char('a') => {
                let target = !self.selected.iter().all(|&s| s);
                self.selected.fill(target);
            }
            KeyCode::Enter => {
                let Some(candidate) = self.current() else {
                    return Step::Stay;
                };
                let indices: Vec<usize> = candidate
                    .open
                    .iter()
                    .zip(&self.selected)
                    .filter(|&(_, &picked)| picked)
                    .map(|((i, _), _)| *i)
                    .collect();
                if indices.is_empty() {
                    return Step::Stay;
                }
                return Step::Pull(Pulled {
                    source: candidate.name.clone(),
                    indices,
                });
            }
            _ => {}
        }
        Step::Stay
    }
}

/// Marker in front of the row the cursor is on. Same glyph the picker uses.
const CURSOR: &str = "\u{25b8}";

/// Column the session rows' open-counts start on, so they line up under each
/// other without the picker's full right-alignment machinery.
const COUNT_COLUMN: usize = 26;

/// Draw whichever stage is current. Chrome is monochrome, like the picker's.
fn draw(frame: &mut Frame, state: &PullState, palette: Palette) {
    let _ = palette;
    let [header, body, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let (title, rows, hint) = if state.is_empty() {
        (
            " caleb \u{2014} pull open tasks".to_string(),
            vec!["  no other sessions have open tasks".to_string()],
            " any key returns".to_string(),
        )
    } else {
        match state.stage {
            Stage::Sessions => (
                " caleb \u{2014} pull open tasks from".to_string(),
                state
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(i, c)| session_row(c, i == state.session_cursor))
                    .collect(),
                " j/k move   Enter choose   Esc cancel".to_string(),
            ),
            Stage::Tasks => {
                let candidate = state.current().expect("a stage-two session exists");
                (
                    format!(
                        " caleb \u{2014} open tasks in {}",
                        picker::pretty_name(&candidate.name)
                    ),
                    candidate
                        .open
                        .iter()
                        .zip(&state.selected)
                        .enumerate()
                        .map(|(i, ((_, text), &picked))| {
                            task_row(text, picked, i == state.task_cursor)
                        })
                        .collect(),
                    format!(
                        " space toggle   a all/none   Enter pull {}   Esc back",
                        state.selected.iter().filter(|&&s| s).count()
                    ),
                )
            }
        }
    };

    frame.render_widget(
        Line::from(title).style(Style::default().add_modifier(Modifier::BOLD)),
        ratatui::layout::Rect {
            height: 1,
            ..header
        },
    );
    frame.render_widget(
        List::new(rows.into_iter().map(ListItem::new).collect::<Vec<_>>()),
        body,
    );
    frame.render_widget(
        Line::from(hint).style(Style::default().add_modifier(Modifier::DIM)),
        status,
    );
}

/// `▸ 2026-05-31  14:30         2 open`. A hand-named session longer than
/// `COUNT_COLUMN` keeps one space before its count rather than running the two
/// halves together, which is what the `.max(1)` is for.
fn session_row(candidate: &Candidate, selected: bool) -> String {
    let marker = if selected { CURSOR } else { " " };
    let name = picker::pretty_name(&candidate.name);
    let pad = COUNT_COLUMN.saturating_sub(name.chars().count());
    format!(
        "{marker} {name}{:pad$}{} open",
        "",
        candidate.open.len(),
        pad = pad.max(1)
    )
}

fn task_row(text: &str, picked: bool, selected: bool) -> String {
    let marker = if selected { CURSOR } else { " " };
    let box_ = if picked { 'x' } else { ' ' };
    format!("{marker} [{box_}] {text}")
}

/// Run the two-stage picker to a decision. `None` means the user cancelled.
///
/// Mouse events are ignored on purpose: a stray click must not toggle a task
/// the user did not mean to include, and there is no drag or scroll here worth
/// the hit-testing.
pub fn run(
    dir: &Path,
    tui: &mut Tui,
    palette: Palette,
    current: &str,
) -> std::io::Result<Option<Pulled>> {
    let entries = picker::scan(dir)?;
    let mut state = PullState::new(&entries, current);

    loop {
        tui.terminal().draw(|frame| draw(frame, &state, palette))?;

        if let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            match state.on_key(key.code) {
                Step::Stay => {}
                Step::Cancel => return Ok(None),
                Step::Pull(pulled) => return Ok(Some(pulled)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(name: &str, contents: &str) -> Entry {
        let counts = markdown::count_tasks(contents);
        Entry {
            name: name.to_string(),
            open: counts.open,
            total: counts.total,
            contents: contents.to_string(),
        }
    }

    const TWO_OPEN: &str = "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] alpha\n- [ ] beta\n\n## Completed\n\n- [x] gamma\n";

    fn state() -> PullState {
        PullState::new(&[entry("2026-05-31_14-30.md", TWO_OPEN)], "current.md")
    }

    fn render(width: u16, height: u16, state: &PullState) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, state, crate::ui::Palette::new(true)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn screen(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_session_stage_lists_names_and_open_counts() {
        let text = screen(&render(70, 8, &state()));
        assert!(text.contains("2026-05-31  14:30"), "{text}");
        assert!(text.contains("2 open"), "{text}");
        assert!(text.contains("Enter choose"), "{text}");
    }

    #[test]
    fn the_task_stage_shows_checkboxes_and_a_live_count() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("[x] alpha"), "{text}");
        assert!(text.contains("[x] beta"), "{text}");
        assert!(text.contains("Enter pull 2"), "{text}");

        s.on_key(KeyCode::Char(' '));
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("[ ] alpha"), "{text}");
        assert!(
            text.contains("Enter pull 1"),
            "the count must follow: {text}"
        );
    }

    #[test]
    fn the_task_stage_headline_names_the_session() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        let text = screen(&render(70, 8, &s));
        assert!(text.contains("2026-05-31  14:30"), "{text}");
    }

    #[test]
    fn an_empty_state_says_so() {
        let text = screen(&render(70, 8, &PullState::new(&[], "current.md")));
        assert!(text.contains("no other sessions have open tasks"), "{text}");
    }

    #[test]
    fn candidates_carry_each_open_tasks_index_and_text() {
        let got = candidates(&[entry("a.md", TWO_OPEN)], "current.md");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a.md");
        assert_eq!(
            got[0].open,
            vec![(0, "alpha".to_string()), (1, "beta".to_string())]
        );
    }

    #[test]
    fn candidates_exclude_the_current_session() {
        assert!(candidates(&[entry("current.md", TWO_OPEN)], "current.md").is_empty());
    }

    #[test]
    fn candidates_exclude_sessions_with_nothing_open() {
        let done = "## Completed\n\n- [x] finished\n";
        assert!(candidates(&[entry("a.md", done)], "current.md").is_empty());
    }

    #[test]
    fn candidates_ignore_an_open_line_filed_under_completed() {
        // `count_tasks` calls this session open; `parse` does not put the task
        // in `active`, so there is nothing here a pull could move.
        let odd = "## Completed\n\n- [ ] hand written in the wrong place\n";
        assert_eq!(markdown::count_tasks(odd).open, 1, "precondition");
        assert!(candidates(&[entry("a.md", odd)], "current.md").is_empty());
    }

    #[test]
    fn candidates_skip_a_task_that_is_done_but_filed_under_active() {
        let odd = "## Active\n\n- [x] already done\n- [ ] real work\n";
        let got = candidates(&[entry("a.md", odd)], "current.md");
        assert_eq!(got[0].open, vec![(1, "real work".to_string())]);
    }

    #[test]
    fn candidates_drop_a_file_that_cannot_be_parsed() {
        let bad = format!("## Active\n\n- [ ] {}\n", "x".repeat(200));
        assert!(candidates(&[entry("bad.md", &bad)], "current.md").is_empty());
    }

    #[test]
    fn enter_on_a_session_opens_its_tasks_all_selected() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Enter), Step::Stay);
        assert_eq!(s.stage, Stage::Tasks);
        assert_eq!(s.selected, vec![true, true]);
        assert_eq!(s.task_cursor, 0);
    }

    #[test]
    fn space_toggles_the_task_under_the_cursor() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![false, true]);
        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![true, true]);
    }

    #[test]
    fn a_selects_everything_unless_everything_is_already_selected() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![false, false], "all selected -> clear");
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![true, true], "any clear -> select all");

        s.on_key(KeyCode::Char(' '));
        assert_eq!(s.selected, vec![false, true], "precondition: a mix");
        s.on_key(KeyCode::Char('a'));
        assert_eq!(s.selected, vec![true, true], "a mix selects all");
    }

    #[test]
    fn enter_pulls_the_selected_indices_in_ascending_order() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char(' ')); // clear index 0
        let step = s.on_key(KeyCode::Enter);
        assert_eq!(
            step,
            Step::Pull(Pulled {
                source: "2026-05-31_14-30.md".to_string(),
                indices: vec![1],
            })
        );
    }

    #[test]
    fn enter_on_an_empty_selection_does_nothing() {
        let mut s = state();
        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Char('a')); // clear all
        assert_eq!(s.on_key(KeyCode::Enter), Step::Stay);
        assert_eq!(s.stage, Stage::Tasks, "and it stays on the screen");
    }

    #[test]
    fn esc_in_the_task_stage_goes_back_with_the_session_cursor_intact() {
        let two = vec![
            entry("2026-05-31_14-30.md", TWO_OPEN),
            entry("2026-05-30_09-00.md", TWO_OPEN),
        ];
        let mut s = PullState::new(&two, "current.md");
        s.on_key(KeyCode::Char('j'));
        assert_eq!(s.session_cursor, 1);

        s.on_key(KeyCode::Enter);
        assert_eq!(s.on_key(KeyCode::Esc), Step::Stay);
        assert_eq!(s.stage, Stage::Sessions);
        assert_eq!(s.session_cursor, 1, "the cursor must not reset");
    }

    #[test]
    fn esc_in_the_session_stage_cancels() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Esc), Step::Cancel);
    }

    #[test]
    fn q_cancels_from_either_stage() {
        let mut s = state();
        assert_eq!(s.on_key(KeyCode::Char('q')), Step::Cancel);

        let mut s = state();
        s.on_key(KeyCode::Enter);
        assert_eq!(s.on_key(KeyCode::Char('q')), Step::Cancel);
    }

    #[test]
    fn cursors_clamp_at_both_ends() {
        let mut s = state();
        s.on_key(KeyCode::Char('k'));
        assert_eq!(s.session_cursor, 0, "already at the top");
        s.on_key(KeyCode::Char('j'));
        assert_eq!(s.session_cursor, 0, "only one session to sit on");

        s.on_key(KeyCode::Enter);
        s.on_key(KeyCode::Down);
        assert_eq!(s.task_cursor, 1);
        s.on_key(KeyCode::Down);
        assert_eq!(s.task_cursor, 1, "two tasks, so index 1 is the end");
        s.on_key(KeyCode::Up);
        s.on_key(KeyCode::Up);
        assert_eq!(s.task_cursor, 0);
    }

    #[test]
    fn every_key_cancels_when_there_is_nothing_to_pull() {
        // The screen has nothing on it to act on, so no key should leave the
        // user stuck looking at it.
        for code in [
            KeyCode::Enter,
            KeyCode::Char('j'),
            KeyCode::Char(' '),
            KeyCode::Esc,
        ] {
            let mut s = PullState::new(&[], "current.md");
            assert_eq!(s.on_key(code), Step::Cancel, "{code:?} must cancel");
        }
    }
}
