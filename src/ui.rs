//! Frame rendering: a pure function from `ViewState` to a ratatui buffer.
//!
//! Rust/ratatui note: ava wrote ANSI escapes by hand and repainted the whole
//! screen every frame. ratatui draws into a cell buffer and diffs it against
//! the previous frame, sending only changed cells. The visible result is the
//! same; the plumbing is gone.

use crate::session::{Pane, Task, Timestamp};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

/// Below this, draw only a "terminal too small" message.
#[allow(dead_code)]
pub const MIN_ROWS: u16 = 8;
#[allow(dead_code)]
pub const MIN_COLS: u16 = 30;

/// Rows each task occupies: one blank spacer plus one content row, so items
/// read as visually separated.
#[allow(dead_code)]
pub const ROW_STRIDE: u16 = 2;

#[allow(dead_code)]
const STATUS_TEXT: &str = " a add  d delete  space toggle  J/K move  s save  q quit  ? help";

/// 256-color palette for chrome. Kept in one place so retuning the look is a
/// one-line change per slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Palette {
    pub accent: Color,
    pub muted: Color,
    pub help: Color,
    pub warn: Color,
    pub color_enabled: bool,
}

#[allow(dead_code)]
impl Palette {
    /// When color is disabled, every slot becomes `Color::Reset` — but
    /// callers keep applying BOLD/DIM/REVERSED/CROSSED_OUT, which are
    /// attributes rather than colors and stay useful without them.
    pub fn new(color_enabled: bool) -> Self {
        if color_enabled {
            Self {
                accent: Color::Indexed(141),
                muted: Color::Indexed(240),
                help: Color::Indexed(177),
                warn: Color::Indexed(221),
                color_enabled,
            }
        } else {
            Self {
                accent: Color::Reset,
                muted: Color::Reset,
                help: Color::Reset,
                warn: Color::Reset,
                color_enabled,
            }
        }
    }

    fn border(self, focused: bool) -> Style {
        Style::default().fg(if focused { self.accent } else { self.muted })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Overlay<'a> {
    None,
    Input(&'a str),
    Help,
}

/// Everything the renderer needs. It never mutates this.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ViewState<'a> {
    pub timestamp: Option<Timestamp>,
    pub active: &'a [Task],
    pub completed: &'a [Task],
    pub focused: Pane,
    pub active_cursor: usize,
    pub completed_cursor: usize,
    pub active_scroll: usize,
    pub completed_scroll: usize,
    pub dirty: bool,
    pub palette: Palette,
    pub overlay: Overlay<'a>,
}

/// Where the panes landed this frame, so mouse events can be hit-tested
/// against the same geometry the user is looking at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PaneRects {
    pub active: Rect,
    pub completed: Rect,
}

/// How many tasks fit in a pane of `pane_height` rows (borders included).
#[allow(dead_code)]
pub fn visible_tasks(pane_height: u16) -> usize {
    let inner = pane_height.saturating_sub(2);
    (inner / ROW_STRIDE) as usize
}

#[allow(dead_code)]
pub fn draw(frame: &mut Frame, state: &ViewState) -> PaneRects {
    let area = frame.area();

    if area.height < MIN_ROWS || area.width < MIN_COLS {
        // Right-aligned: on an extremely narrow terminal (the message is 25
        // cols; MIN_COLS only guarantees 30), left alignment would clip the
        // tail ("small") off first. Right alignment clips the "caleb: "
        // branding first instead, keeping the actual diagnosis on screen.
        frame.render_widget(
            Line::from("caleb: terminal too small").alignment(Alignment::Right),
            area,
        );
        return PaneRects::default();
    }

    // The input overlay is a 3-row bordered field plus a 1-row spacer. The
    // spacer keeps the bottom border clear of tmux/powerline status lines
    // that overlay the last row.
    let bottom = match state.overlay {
        Overlay::Input(_) => 4,
        Overlay::None | Overlay::Help => 1,
    };

    let [header_area, panes_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(bottom),
    ])
    .areas(area);

    draw_header(frame, state, header_area);
    let rects = draw_panes(frame, state, panes_area);

    match state.overlay {
        Overlay::None => draw_status_bar(frame, state, bottom_area),
        Overlay::Input(buf) => draw_input_bar(frame, state, buf, bottom_area),
        Overlay::Help => {
            draw_status_bar(frame, state, bottom_area);
            draw_help_overlay(frame, state, area);
        }
    }

    rects
}

fn draw_header(frame: &mut Frame, state: &ViewState, area: Rect) {
    let mut spans = vec![Span::styled(
        " caleb ",
        Style::default().add_modifier(Modifier::BOLD),
    )];

    if let Some(ts) = state.timestamp {
        spans.push(Span::styled(
            format!(
                "· {:04}-{:02}-{:02} {:02}:{:02} ",
                ts.year, ts.month, ts.day, ts.hour, ts.minute
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::styled(
        format!(
            "· {} active / {} done",
            state.active.len(),
            state.completed.len()
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ));

    if state.dirty {
        spans.push(Span::styled(
            "  •unsaved",
            Style::default().fg(state.palette.warn),
        ));
    }

    frame.render_widget(Line::from(spans), area);
}

fn draw_panes(frame: &mut Frame, state: &ViewState, area: Rect) -> PaneRects {
    // Left gets floor(width/2); the odd column lands in the right pane,
    // matching ava's `cols / 2` + remainder split exactly.
    let [left, right] =
        Layout::horizontal([Constraint::Length(area.width / 2), Constraint::Min(0)]).areas(area);

    draw_pane(frame, state, left, Pane::Active);
    draw_pane(frame, state, right, Pane::Completed);

    PaneRects {
        active: left,
        completed: right,
    }
}

fn draw_pane(frame: &mut Frame, state: &ViewState, area: Rect, pane: Pane) {
    let focused = state.focused == pane;
    let (tasks, cursor, scroll, title) = match pane {
        Pane::Active => (
            state.active,
            state.active_cursor,
            state.active_scroll,
            "─ Active ",
        ),
        Pane::Completed => (
            state.completed,
            state.completed_cursor,
            state.completed_scroll,
            "─ Completed ",
        ),
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(state.palette.border(focused))
        .padding(Padding::horizontal(1))
        .title(title);

    // Each item is two lines: a blank spacer, then the content. The spacer
    // comes first so the top item gets the same gap from the border that
    // separates adjacent items.
    //
    // Styling is applied per-Line rather than via `List::highlight_style`,
    // which would paint the spacer row too and give a 2-row highlight where
    // ava has 1.
    let items: Vec<ListItem> = tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let marker = if pane == Pane::Completed {
                "✓ "
            } else {
                "  "
            };
            let mut style = Style::default();
            if pane == Pane::Completed {
                style = style.add_modifier(Modifier::DIM | Modifier::CROSSED_OUT);
            }
            if focused && i == cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(vec![
                Line::from(""),
                Line::from(format!("{marker}{}", task.text)).style(style),
            ])
        })
        .collect();

    let mut list_state = ListState::default();
    *list_state.offset_mut() = scroll;
    frame.render_stateful_widget(List::new(items).block(block), area, &mut list_state);
}

fn draw_status_bar(frame: &mut Frame, _state: &ViewState, area: Rect) {
    frame.render_widget(
        Line::from(STATUS_TEXT).style(Style::default().add_modifier(Modifier::DIM)),
        area,
    );
}

fn draw_input_bar(_f: &mut Frame, _s: &ViewState, _buf: &str, _a: Rect) {}
fn draw_help_overlay(_f: &mut Frame, _s: &ViewState, _a: Rect) {}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn task(text: &str, done: bool) -> Task {
        Task {
            text: text.to_string(),
            done,
        }
    }

    fn render(width: u16, height: u16, state: &ViewState) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                draw(f, state);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn base<'a>(active: &'a [Task], completed: &'a [Task]) -> ViewState<'a> {
        ViewState {
            timestamp: None,
            active,
            completed,
            focused: Pane::Active,
            active_cursor: 0,
            completed_cursor: 0,
            active_scroll: 0,
            completed_scroll: 0,
            dirty: false,
            palette: Palette::new(true),
            overlay: Overlay::None,
        }
    }

    #[test]
    fn too_small_terminal_shows_fallback() {
        let buf = render(20, 4, &base(&[], &[]));
        assert!(row(&buf, 0).contains("terminal too small"));
    }

    #[test]
    fn header_shows_name_timestamp_and_counts() {
        let mut s = base(&[], &[]);
        s.timestamp = Some(Timestamp {
            year: 2026,
            month: 5,
            day: 31,
            hour: 14,
            minute: 30,
        });
        let buf = render(60, 10, &s);
        assert_eq!(
            row(&buf, 0).trim_end(),
            " caleb · 2026-05-31 14:30 · 0 active / 0 done"
        );
    }

    #[test]
    fn header_shows_unsaved_marker_when_dirty() {
        let mut s = base(&[], &[]);
        s.dirty = true;
        let buf = render(60, 10, &s);
        assert!(row(&buf, 0).contains("•unsaved"));
    }

    #[test]
    fn pane_borders_carry_ava_titles() {
        let buf = render(40, 10, &base(&[], &[]));
        assert_eq!(row(&buf, 1), "╭─ Active ─────────╮╭─ Completed ──────╮");
    }

    #[test]
    fn tasks_render_with_spacer_above_and_markers() {
        let active = [task("first thing", false)];
        let completed = [task("done thing", true)];
        let buf = render(40, 10, &base(&active, &completed));
        // Row 2 is the spacer, row 3 the first task's content.
        assert_eq!(row(&buf, 2), "│                  ││                  │");
        assert!(row(&buf, 3).contains("  first thing"));
        assert!(row(&buf, 3).contains("✓ done thing"));
    }

    #[test]
    fn cursor_row_is_reversed_and_spacer_is_not() {
        let active = [task("first thing", false)];
        let buf = render(40, 10, &base(&active, &[]));
        assert!(buf[(3u16, 3u16)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3u16, 2u16)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn completed_tasks_are_dim_and_struck_through() {
        let completed = [task("done thing", true)];
        let mut s = base(&[], &completed);
        s.focused = Pane::Active;
        let buf = render(40, 10, &s);
        let cell = &buf[(22u16, 3u16)];
        assert!(cell.modifier.contains(Modifier::DIM));
        assert!(cell.modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn focused_pane_uses_accent_and_unfocused_uses_muted() {
        let buf = render(40, 10, &base(&[], &[]));
        assert_eq!(buf[(0u16, 1u16)].fg, Color::Indexed(141));
        assert_eq!(buf[(20u16, 1u16)].fg, Color::Indexed(240));
    }

    #[test]
    fn status_bar_matches_ava() {
        let buf = render(70, 10, &base(&[], &[]));
        assert_eq!(
            row(&buf, 9).trim_end(),
            " a add  d delete  space toggle  J/K move  s save  q quit  ? help"
        );
    }

    #[test]
    fn odd_width_gives_the_extra_column_to_the_right_pane() {
        let buf = render(41, 10, &base(&[], &[]));
        let line = row(&buf, 1);
        // Left pane is 20 cols, right pane 21.
        assert_eq!(line.chars().nth(20).unwrap(), '╭');
    }

    #[test]
    fn no_color_drops_colors_but_keeps_attributes() {
        let active = [task("first thing", false)];
        let mut s = base(&active, &[]);
        s.palette = Palette::new(false);
        let buf = render(40, 10, &s);
        // Border color gone...
        assert_eq!(buf[(0u16, 1u16)].fg, Color::Reset);
        // ...but the cursor is still visible.
        assert!(buf[(3u16, 3u16)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn scroll_offset_selects_the_first_visible_task() {
        let active: Vec<Task> = (0..10).map(|i| task(&format!("task{i}"), false)).collect();
        let mut s = base(&active, &[]);
        s.active_scroll = 4;
        s.active_cursor = 4;
        let buf = render(40, 10, &s);
        assert!(row(&buf, 3).contains("task4"));
    }

    #[test]
    fn visible_tasks_halves_the_inner_height() {
        // A 10-row terminal: 1 header + 1 status = 8 pane rows, 6 inner.
        assert_eq!(visible_tasks(8), 3);
        assert_eq!(visible_tasks(2), 0);
    }
}
