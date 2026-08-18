//! Frame rendering: a pure function from `ViewState` to a ratatui buffer.
//!
//! Rust/ratatui note: nothing here writes ANSI escapes. ratatui draws into a
//! cell buffer and diffs it against the previous frame, sending only changed
//! cells — which is also why tests can assert against a `TestBackend` buffer
//! instead of a real terminal.

use crate::model::{Pane, Task, Timestamp};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding, Paragraph};
use std::time::{Duration, Instant};

/// Below this, draw only a "terminal too small" message.
pub const MIN_ROWS: u16 = 10;
pub const MIN_COLS: u16 = 30;

/// Rows each task occupies: one blank spacer plus one content row, so items
/// read as visually separated.
pub const ROW_STRIDE: u16 = 2;

/// Two clicks on the same target within this window count as a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Tracks repeat clicks on the same target. `K` identifies whatever "the same
/// target" means to the caller — a row index in the picker, a (pane, index)
/// pair in the main view.
///
/// Rust note: `saturating_duration_since` rather than `now - t`, because
/// `Instant` subtraction panics when the result would be negative. A monotonic
/// clock makes that unreachable, but stating it costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickTracker<K> {
    last: Option<(Instant, K)>,
}

/// Written out rather than derived: `#[derive(Default)]` would add a spurious
/// `K: Default` bound, and `Pane` has no sensible default.
impl<K> Default for ClickTracker<K> {
    fn default() -> Self {
        Self { last: None }
    }
}

impl<K: Copy + PartialEq> ClickTracker<K> {
    /// Record a click on `key`, returning `true` if it completes a double
    /// click. A completed double click resets the tracker, so three rapid
    /// clicks register as one double click and one single, not two.
    pub fn click(&mut self, key: K, now: Instant) -> bool {
        let is_double = self
            .last
            .is_some_and(|(t, k)| k == key && now.saturating_duration_since(t) <= DOUBLE_CLICK);
        self.last = if is_double { None } else { Some((now, key)) };
        is_double
    }

    /// Forget the pending click, so the next one cannot pair with it.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

const STATUS_TEXT: &str = " a add  d delete  space toggle  J/K move  s save  q quit  ? help";

/// Heavy verticals span terminal cell boundaries more reliably than `│`,
/// while light horizontals preserve the existing pane appearance.
const PANE_BORDER: border::Set<'static> = border::Set {
    top_left: "┍",
    top_right: "┑",
    bottom_left: "┕",
    bottom_right: "┙",
    vertical_left: "┃",
    vertical_right: "┃",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

/// 256-color palette for chrome. Kept in one place so retuning the look is a
/// one-line change per slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub muted: Color,
    pub help: Color,
    pub warn: Color,
    pub color_enabled: bool,
}

impl Palette {
    /// When color is disabled, every slot becomes `Color::Reset` — but
    /// callers keep applying BOLD/DIM/REVERSED/CROSSED_OUT, which are
    /// attributes rather than colors and stay useful without them.
    pub fn new(color_enabled: bool) -> Self {
        if color_enabled {
            Self {
                accent: Color::Indexed(40),
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
pub enum Overlay<'a> {
    None,
    Input(&'a str),
    Help,
}

/// Everything the renderer needs. It never mutates this.
#[derive(Debug, Clone, Copy)]
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
pub struct PaneRects {
    pub active: Rect,
    pub completed: Rect,
}

/// How many tasks fit in a pane of `pane_height` rows (borders included).
pub fn visible_tasks(pane_height: u16) -> usize {
    let inner = pane_height.saturating_sub(2);
    (inner / ROW_STRIDE) as usize
}

pub fn draw(frame: &mut Frame, state: &ViewState) -> PaneRects {
    let area = frame.area();

    if area.height < MIN_ROWS || area.width < MIN_COLS {
        frame.render_widget(Line::from("caleb: terminal too small"), area);
        return PaneRects::default();
    }

    // The input overlay is a 3-row bordered field plus a 1-row spacer. The
    // spacer keeps the bottom border clear of tmux/powerline status lines
    // that overlay the last row.
    let bottom = match state.overlay {
        Overlay::Input(_) => 4,
        Overlay::None | Overlay::Help => 1,
    };

    // One blank row above and below the header gives the top line a little
    // breathing room before the pane borders.
    let [_, header_area, _, panes_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
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
            format!("· {ts} "),
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
    // Left gets floor(width/2); the odd column lands in the right pane.
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
        .border_set(PANE_BORDER)
        .border_style(state.palette.border(focused))
        .padding(Padding::horizontal(1))
        .title(title);

    // Each item is two lines: a blank spacer, then the content. The spacer
    // comes first so the top item gets the same gap from the border that
    // separates adjacent items.
    //
    // Styling is applied per-Line rather than via `List::highlight_style`,
    // which would paint the spacer row too and give a 2-row highlight instead
    // of 1.
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

/// Key reference shown by `?`. Each line is padded to 52 columns so the box
/// has a straight right edge.
pub const HELP_LINES: &[&str] = &[
    " caleb — key reference                              ",
    "                                                    ",
    " Navigation                                         ",
    "   j / ↓        next task in focused pane           ",
    "   k / ↑        prev task in focused pane           ",
    "   h / ← →/ l   switch pane                         ",
    "   g / G        top / bottom of pane                ",
    "                                                    ",
    " Editing                                            ",
    "   a            add task (input at the bottom)      ",
    "   d            delete selected task                ",
    "   space / x    toggle done (moves task)            ",
    "   Shift+J / K  move task down / up                 ",
    "                                                    ",
    " Mouse                                              ",
    "   click        select task / focus pane            ",
    "   double click toggle done                         ",
    "   wheel        scroll focused pane                 ",
    "                                                    ",
    " App                                                ",
    "   s            save                                ",
    "   q            quit (auto-saves)                   ",
    "   ?            this help                           ",
    "   Esc          dismiss / cancel                    ",
];

const HELP_INNER_W: u16 = 52;

/// Three-row bordered field with a one-row spacer underneath:
///
/// ```text
/// row rows-3   ╭─ Add task ──...──╮
/// row rows-2   │ > <buf>_<padding>│
/// row rows-1   ╰──────...─────────╯
/// row rows     (blank, clears tmux/powerline overlay bars)
/// ```
fn draw_input_bar(frame: &mut Frame, state: &ViewState, buf: &str, area: Rect) {
    let field = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 3,
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(state.palette.accent))
        .title("─ Add task ");

    let inner = block.inner(field);
    frame.render_widget(block, field);

    // Truncate from the left so the caret stays visible on long input.
    // Rust note: `buf.len() - room` is a byte offset, and slicing a &str at
    // an offset inside a multi-byte character panics. Walking forward to the
    // next char boundary keeps the slice valid and never exceeds `room`.
    // 4 reserved columns: " > " (3) plus the trailing "_" caret (1). Using 3
    // here left no room for the caret, so it silently fell off the right
    // edge whenever `shown` filled the full budget.
    let room = inner.width.saturating_sub(4) as usize;
    let shown = if buf.len() > room {
        let mut start = buf.len() - room;
        while start < buf.len() && !buf.is_char_boundary(start) {
            start += 1;
        }
        &buf[start..]
    } else {
        buf
    };

    frame.render_widget(
        Line::from(vec![
            Span::styled(" > ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(shown),
            Span::raw("_"),
        ]),
        inner,
    );
}

/// Centered, with one row of vertical and one column of horizontal padding
/// between the text and the border.
fn draw_help_overlay(frame: &mut Frame, state: &ViewState, area: Rect) {
    let box_w = HELP_INNER_W + 2 + 2; // padding + borders
    let box_h = HELP_LINES.len() as u16 + 2 + 2;

    // Needs a 1-cell margin on every side.
    if area.width < box_w + 2 || area.height < box_h + 2 {
        return;
    }

    let rect = Rect {
        x: area.x + (area.width - box_w) / 2,
        y: area.y + (area.height - box_h) / 2,
        width: box_w,
        height: box_h,
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(state.palette.help))
        .padding(Padding::uniform(1));

    let lines: Vec<Line> = HELP_LINES.iter().map(|l| Line::from(*l)).collect();

    // `Clear` blanks the cells first so the panes underneath do not show
    // through the overlay.
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

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
    fn click_tracker_pairs_only_same_target_within_the_window() {
        let t0 = Instant::now();
        let mut tracker = ClickTracker::default();

        assert!(!tracker.click(1usize, t0), "first click is never a double");
        assert!(tracker.click(1usize, t0 + Duration::from_millis(100)));

        // Same target, but too slow.
        assert!(!tracker.click(2usize, t0));
        assert!(!tracker.click(2usize, t0 + DOUBLE_CLICK + Duration::from_millis(1)));

        // Fast enough, but a different target.
        assert!(!tracker.click(3usize, t0));
        assert!(!tracker.click(4usize, t0 + Duration::from_millis(10)));
    }

    #[test]
    fn click_tracker_resets_after_a_double_so_triples_do_not_chain() {
        let t0 = Instant::now();
        let mut tracker = ClickTracker::default();
        assert!(!tracker.click(1usize, t0));
        assert!(tracker.click(1usize, t0 + Duration::from_millis(10)));
        // Third rapid click starts a new pair rather than firing again.
        assert!(!tracker.click(1usize, t0 + Duration::from_millis(20)));
    }

    #[test]
    fn click_tracker_reset_breaks_the_pending_pair() {
        let t0 = Instant::now();
        let mut tracker = ClickTracker::default();
        assert!(!tracker.click(1usize, t0));
        tracker.reset();
        assert!(!tracker.click(1usize, t0 + Duration::from_millis(10)));
    }

    #[test]
    fn too_small_terminal_shows_fallback() {
        let buf = render(40, 4, &base(&[], &[]));
        assert!(row(&buf, 0).contains("terminal too small"));
    }

    #[test]
    fn too_small_terminal_left_clips_when_narrow() {
        let buf = render(20, 4, &base(&[], &[]));
        assert!(row(&buf, 0).starts_with("caleb: terminal too"));
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
            row(&buf, 1).trim_end(),
            " caleb · 2026-05-31 14:30 · 0 active / 0 done"
        );
    }

    #[test]
    fn header_shows_unsaved_marker_when_dirty() {
        let mut s = base(&[], &[]);
        s.dirty = true;
        let buf = render(60, 10, &s);
        assert!(row(&buf, 1).contains("•unsaved"));
    }

    #[test]
    fn pane_borders_carry_titles_with_mixed_weight_corners() {
        let buf = render(40, 10, &base(&[], &[]));
        assert_eq!(row(&buf, 3), "┍─ Active ─────────┑┍─ Completed ──────┑");
    }

    #[test]
    fn pane_vertical_borders_are_continuous() {
        let buf = render(40, 10, &base(&[], &[]));
        for y in 4..8 {
            for x in [0, 19, 20, 39] {
                assert_eq!(buf[(x, y)].symbol(), "┃");
            }
        }
    }

    #[test]
    fn tasks_render_with_spacer_above_and_markers() {
        let active = [task("first thing", false)];
        let completed = [task("done thing", true)];
        let buf = render(40, 10, &base(&active, &completed));
        // Row 4 is the spacer, row 5 the first task's content.
        assert_eq!(row(&buf, 4), "┃                  ┃┃                  ┃");
        assert!(row(&buf, 5).contains("  first thing"));
        assert!(row(&buf, 5).contains("✓ done thing"));
    }

    #[test]
    fn cursor_row_is_reversed_and_spacer_is_not() {
        let active = [task("first thing", false)];
        let buf = render(40, 10, &base(&active, &[]));
        assert!(buf[(3u16, 5u16)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3u16, 4u16)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn completed_tasks_are_dim_and_struck_through() {
        let completed = [task("done thing", true)];
        let mut s = base(&[], &completed);
        s.focused = Pane::Active;
        let buf = render(40, 10, &s);
        let cell = &buf[(22u16, 5u16)];
        assert!(cell.modifier.contains(Modifier::DIM));
        assert!(cell.modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn focused_pane_uses_accent_and_unfocused_uses_muted() {
        let buf = render(40, 10, &base(&[], &[]));
        assert_eq!(buf[(0u16, 3u16)].fg, Color::Indexed(40));
        assert_eq!(buf[(20u16, 3u16)].fg, Color::Indexed(240));
    }

    #[test]
    fn status_bar_lists_every_binding() {
        let buf = render(70, 10, &base(&[], &[]));
        assert_eq!(
            row(&buf, 9).trim_end(),
            " a add  d delete  space toggle  J/K move  s save  q quit  ? help"
        );
    }

    #[test]
    fn odd_width_gives_the_extra_column_to_the_right_pane() {
        let buf = render(41, 10, &base(&[], &[]));
        let line = row(&buf, 3);
        // Left pane is 20 cols, right pane 21.
        assert_eq!(line.chars().nth(20).unwrap(), '┍');
    }

    #[test]
    fn no_color_drops_colors_but_keeps_attributes() {
        let active = [task("first thing", false)];
        let mut s = base(&active, &[]);
        s.palette = Palette::new(false);
        let buf = render(40, 10, &s);
        // Border color gone...
        assert_eq!(buf[(0u16, 3u16)].fg, Color::Reset);
        // ...but the cursor is still visible.
        assert!(buf[(3u16, 5u16)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn scroll_offset_selects_the_first_visible_task() {
        let active: Vec<Task> = (0..10).map(|i| task(&format!("task{i}"), false)).collect();
        let mut s = base(&active, &[]);
        s.active_scroll = 4;
        s.active_cursor = 4;
        let buf = render(40, 10, &s);
        assert!(row(&buf, 5).contains("task4"));
    }

    #[test]
    fn visible_tasks_halves_the_inner_height() {
        // A 12-row terminal: 3 header rows + 1 status = 8 pane rows, 6 inner.
        assert_eq!(visible_tasks(8), 3);
        assert_eq!(visible_tasks(2), 0);
    }

    #[test]
    fn input_overlay_draws_a_bordered_field() {
        let mut s = base(&[], &[]);
        s.overlay = Overlay::Input("hi");
        let buf = render(40, 10, &s);
        // 10 rows, 0-indexed: blank 0, header 1, blank 2, panes 3..=5,
        // field 6/7/8, blank 9.
        // The pane bottom border sits on row 5.
        assert!(row(&buf, 5).starts_with('┕'));
        assert!(row(&buf, 6).starts_with("╭─ Add task "));
        assert!(row(&buf, 7).contains("> hi_"));
        assert!(row(&buf, 8).starts_with('╰'));
        assert_eq!(row(&buf, 9).trim(), "");
    }

    #[test]
    fn input_overlay_hides_the_status_bar() {
        let mut s = base(&[], &[]);
        s.overlay = Overlay::Input("");
        let buf = render(40, 12, &s);
        for y in 0..12 {
            assert!(!row(&buf, y).contains("a add  d delete"));
        }
    }

    #[test]
    fn input_field_uses_the_accent_color() {
        let mut s = base(&[], &[]);
        s.overlay = Overlay::Input("x");
        let buf = render(40, 10, &s);
        // Field top-left corner: row 6, column 0.
        assert_eq!(buf[(0u16, 6u16)].fg, Color::Indexed(40));
    }

    #[test]
    fn help_overlay_is_centered_and_orchid() {
        let mut s = base(&[], &[]);
        s.overlay = Overlay::Help;
        let buf = render(80, 40, &s);
        let joined: String = (0..40).map(|y| row(&buf, y)).collect();
        assert!(joined.contains("caleb — key reference"));
        assert!(joined.contains("Mouse"));
        // Box is 56 wide (52 inner + 2 padding + 2 border) and 28 tall
        // (24 lines + 2 padding + 2 border). On an 80x40 screen it centers
        // at column (80-56)/2 = 12, row (40-28)/2 = 6.
        assert_eq!(buf[(12u16, 6u16)].fg, Color::Indexed(177));
    }

    #[test]
    fn help_overlay_is_skipped_when_it_cannot_fit() {
        let mut s = base(&[], &[]);
        s.overlay = Overlay::Help;
        let buf = render(40, 10, &s);
        let joined: String = (0..10).map(|y| row(&buf, y)).collect();
        assert!(!joined.contains("key reference"));
    }

    #[test]
    fn input_overflow_scrolls_without_splitting_multibyte_chars() {
        // room = inner_width - 4 = (40 - 2) - 4 = 34 visible bytes, and the
        // truncation point is buf.len() - room. Growing the tail by one byte
        // slides that point one byte further past the 'é', so this sweep walks
        // it straight through the middle of the character — the case that used
        // to panic. (It lands mid-character exactly at tail == 33.)
        for tail in 30..40 {
            let text = format!("{}é{}", "x".repeat(20), "y".repeat(tail));
            let mut s = base(&[], &[]);
            s.overlay = Overlay::Input(&text);
            let buf = render(40, 10, &s);
            let content = row(&buf, 7);
            assert!(
                content.contains('_'),
                "tail={tail}: caret should stay visible, got {content:?}"
            );
            assert!(
                !content.contains('\u{fffd}'),
                "tail={tail}: truncation split a character, got {content:?}"
            );
        }
    }
}
