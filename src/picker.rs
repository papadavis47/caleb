//! Resume picker: one screen to choose a past session.

use crate::markdown::count_tasks;
use crate::storage::FILE_EXTENSION;
use crate::tui::Tui;
use crate::ui::{ClickTracker, Palette};
use crossterm::event::{
    self, Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap};
use std::path::Path;
use std::time::Instant;

/// First screen row occupied by a session entry; the 2-row header sits above.
const FIRST_ROW: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub open: u32,
    pub total: u32,
    /// The file as read, kept for the preview pane. `scan` reads every file
    /// to count its tasks, so holding on to the text costs no extra I/O.
    pub contents: String,
}

/// Everything the picker's renderer needs. Grouped rather than passed loose
/// so `draw` keeps one parameter as the screen grows, the way `ui::ViewState`
/// already does for the session screen.
#[derive(Debug, Clone, Copy)]
pub struct PickerView<'a> {
    pub visible: &'a [&'a Entry],
    pub cursor: usize,
    pub show_all: bool,
    pub pending: Option<usize>,
    /// Sessions the filter is holding back, for the empty state.
    pub hidden: usize,
    pub preview_on: bool,
    pub preview_scroll: usize,
    pub palette: Palette,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Selected(String),
    Cancelled,
}

/// Scan `dir` for session files, newest first. Given the
/// `YYYY-MM-DD_HH-MM` format, that is lexical descending.
pub fn scan(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(FILE_EXTENSION) {
            continue;
        }
        // A file deleted between listing and reading is skipped, not fatal.
        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let counts = count_tasks(&contents);
        out.push(Entry {
            name,
            open: counts.open,
            total: counts.total,
            contents,
        });
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

/// Sessions with unfinished work, unless `show_all`.
///
/// Rust note: returning a `Vec<&Entry>` borrows instead of copying and leaves
/// the input untouched — no in-place partition, and the borrow checker proves
/// the entries outlive the filtered view.
pub fn filter_visible(entries: &[Entry], show_all: bool) -> Vec<&Entry> {
    entries.iter().filter(|e| show_all || e.open > 0).collect()
}

/// Columns the list keeps for itself when the preview sits beside it. The
/// widest a row can render — a collision suffix and three-digit counts —
/// is 43, so rows never truncate.
const LIST_WIDTH: u16 = 44;

/// Width the list lays its rows out in: the pane less a matching gutter, so
/// the counts stop the same distance from the divider as the preview text
/// starts from it.
const ROW_WIDTH: u16 = LIST_WIDTH - GUTTER;

/// Columns of breathing room on either side of the divider — the list's
/// counts stop this far short of it, the preview's text starts this far past
/// it. One constant so the two sides cannot drift apart.
const GUTTER: u16 = 3;

/// Below this the preview is dropped entirely and the list runs full width,
/// rather than squeezing both into a column that suits neither.
const MIN_WIDTH_FOR_PREVIEW: u16 = 80;

/// Style one raw line of a session file. Only the three shapes caleb writes
/// are recognised; everything else — including notes hand-added to a file,
/// which `markdown::parse` discards — is dimmed but still shown verbatim.
fn preview_line(raw: &str, palette: Palette) -> Style {
    if raw.starts_with("# ") {
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD)
    } else if raw.starts_with("## ") {
        Style::default()
            .fg(palette.help)
            .add_modifier(Modifier::BOLD)
    } else if raw.starts_with("- [x] ") || raw.starts_with("- [X] ") {
        Style::default()
            .fg(palette.muted)
            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT)
    } else if raw.starts_with("- [ ] ") {
        Style::default()
    } else {
        Style::default()
            .fg(palette.muted)
            .add_modifier(Modifier::DIM)
    }
}

/// Rows `contents` occupies once wrapped into `width` columns.
///
/// A character-count ceiling, not a word-wrap simulation: ratatui breaks on
/// word boundaries, so this can undercount a line by a row. It exists only to
/// stop scrolling somewhere near the end rather than into empty space, and it
/// is exact for the common case of lines that fit.
pub fn wrapped_lines(contents: &str, width: u16) -> usize {
    if width == 0 {
        return contents.lines().count();
    }
    let width = width as usize;
    contents
        .lines()
        .map(|l| l.chars().count().div_ceil(width).max(1))
        .sum()
}

/// One list row: name on the left, counts flush right.
///
/// Laid out in a fixed `ROW_WIDTH` rather than the pane's actual width, so
/// the divider never moves when the visible set changes — and so the counts
/// do not drift to the far edge of a wide terminal when the preview is off.
/// A name too long to align keeps one space before its counts instead of
/// running into them.
fn row_text(entry: &Entry, selected: bool) -> String {
    let marker = if selected { "▸ " } else { "  " };
    let left = format!("{marker}{}", pretty_name(&entry.name));
    let right = format!("{} open / {} total", entry.open, entry.total);
    let used = left.chars().count() + right.chars().count();
    let gap = (ROW_WIDTH as usize).saturating_sub(used).max(1);
    format!("{left}{}{right}", " ".repeat(gap))
}

pub fn preview_fits(width: u16) -> bool {
    width >= MIN_WIDTH_FOR_PREVIEW
}

/// Furthest useful scroll offset: the one that puts the last line on the
/// bottom row. Content that fits never scrolls.
///
/// Rust note: `saturating_sub` matters on both halves — `lines - height`
/// wraps around on unsigned math when the pane is taller than the content,
/// which would send the view far past the end instead of to the top.
pub fn clamp_scroll(scroll: usize, lines: usize, height: usize) -> usize {
    scroll.min(lines.saturating_sub(height))
}

/// Whether to reveal finished sessions when the picker opens, so a store that
/// holds nothing unfinished does not present an empty box.
///
/// This is a decision about the *opening* frame, deliberately not re-asked
/// each iteration: once the picker is up, the visible set changes only when
/// the user asks it to. Re-evaluating it in the loop meant deleting the last
/// unfinished session silently unhid every finished one.
pub fn show_all_on_open(entries: &[Entry]) -> bool {
    !entries.is_empty() && filter_visible(entries, false).is_empty()
}

/// `2026-05-31_14-30.md` becomes `2026-05-31  14:30`. Anything that does
/// not match the format prints verbatim.
///
/// Rust note: the separator checks pin positions 4/7/10/13 to ASCII, but not
/// 16 — a name whose minute field runs into a multi-byte character would
/// make `&stem[14..16]` slice mid-sequence and panic. `is_char_boundary`
/// turns that crash into a pass-through.
pub fn pretty_name(name: &str) -> String {
    let stem = name.strip_suffix(FILE_EXTENSION).unwrap_or(name);
    let b = stem.as_bytes();
    if stem.len() >= 16
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'_'
        && b[13] == b'-'
        && stem.is_char_boundary(16)
    {
        return format!(
            "{}  {}:{}{}",
            &stem[0..10],
            &stem[11..13],
            &stem[14..16],
            &stem[16..]
        );
    }
    name.to_string()
}

/// Outcome of a keypress while the delete confirmation is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Delete,
    Cancel,
}

/// Only `y` goes through with it. Everything else cancels — a whitelist of
/// cancel keys would let an unrecognized keypress fall through to the delete,
/// which is the wrong way round for the one destructive key in the app.
pub fn confirm_key(code: KeyCode) -> Confirm {
    match code {
        KeyCode::Char('y') => Confirm::Delete,
        _ => Confirm::Cancel,
    }
}

/// Remove one session file. The picker re-scans afterwards, so a failure here
/// is reported to the caller but never fatal: either way the next frame shows
/// what is actually on disk.
pub fn delete_entry(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir.join(name))
}

pub fn run(dir: &Path, tui: &mut Tui, palette: Palette) -> std::io::Result<Choice> {
    let mut entries = scan(dir)?;
    let mut cursor = 0usize;
    let mut show_all = show_all_on_open(&entries);
    let mut pending: Option<usize> = None;
    let mut preview_on = true;
    let mut preview_scroll = 0usize;
    let mut last_click = ClickTracker::default();

    loop {
        let visible = filter_visible(&entries, show_all);
        if cursor >= visible.len() && !visible.is_empty() {
            cursor = visible.len() - 1;
        }
        // A prompt left pointing past the end of the list — the row went away
        // underneath it — is dropped rather than aimed at whatever slid up.
        if pending.is_some_and(|i| i >= visible.len()) {
            pending = None;
        }

        tui.terminal().draw(|frame| {
            draw(
                frame,
                &PickerView {
                    visible: &visible,
                    cursor,
                    show_all,
                    pending,
                    hidden: entries.len() - visible.len(),
                    preview_on,
                    preview_scroll,
                    palette,
                },
            );
        })?;

        // Preview geometry from the same frame the user is looking at, so a
        // resize between keypresses cannot scroll against a stale height.
        let size = tui.terminal().size()?;
        let page = size.height.saturating_sub(3) as usize;
        let preview_width = size.width.saturating_sub(LIST_WIDTH + 1 + GUTTER);
        let preview_height = visible
            .get(cursor)
            .map_or(0, |e| wrapped_lines(&e.contents, preview_width));
        let before = cursor;

        // The name is cloned out of `visible` so the borrow of `entries` ends
        // with this iteration and the re-scan below can reassign it.
        let mut confirmed: Option<String> = None;

        match event::read()? {
            Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                if let Some(index) = pending.take() {
                    if confirm_key(key.code) == Confirm::Delete {
                        confirmed = visible.get(index).map(|e| e.name.clone());
                    }
                } else {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(Choice::Cancelled),
                        KeyCode::Down | KeyCode::Char('j') => {
                            if cursor + 1 < visible.len() {
                                cursor += 1;
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
                        KeyCode::Char('a') => {
                            show_all = !show_all;
                            cursor = 0;
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            preview_scroll =
                                clamp_scroll(preview_scroll + page / 2, preview_height, page);
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            preview_scroll = preview_scroll.saturating_sub(page / 2);
                        }
                        KeyCode::Char('d') => {
                            if cursor < visible.len() {
                                pending = Some(cursor);
                            }
                        }
                        KeyCode::Char('p') => preview_on = !preview_on,
                        KeyCode::Enter => {
                            if let Some(e) = visible.get(cursor) {
                                return Ok(Choice::Selected(e.name.clone()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            // A click while the prompt is up would be an answer the user did
            // not mean to give, so the mouse is inert until it is dismissed.
            Event::Mouse(m) if pending.is_none() => {
                if let Some(choice) =
                    handle_mouse(m, &visible, &mut cursor, &mut last_click, Instant::now())
                {
                    return Ok(choice);
                }
            }
            _ => {}
        }

        // Moving to another session shows another document; keeping the old
        // offset would drop the user into the middle of it.
        if cursor != before {
            preview_scroll = 0;
        }

        if let Some(name) = confirmed {
            // The cursor is deliberately left where it is: the next session
            // slides up under it. A failed delete is swallowed because the
            // re-scan reports the truth either way.
            let _ = delete_entry(dir, &name);
            entries = scan(dir)?;
            preview_scroll = 0;
        }
    }
}

fn handle_mouse(
    m: MouseEvent,
    visible: &[&Entry],
    cursor: &mut usize,
    last_click: &mut ClickTracker<usize>,
    now: Instant,
) -> Option<Choice> {
    if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
        return None;
    }
    if m.row < FIRST_ROW {
        return None;
    }
    let index = (m.row - FIRST_ROW) as usize;
    let entry = visible.get(index)?;

    let is_double = last_click.click(index, now);
    *cursor = index;

    is_double.then(|| Choice::Selected(entry.name.clone()))
}

/// The right-hand pane: one session file, styled and scrolled.
///
/// Takes the entry rather than the whole view so it stays callable with
/// nothing selected, which is what an empty list renders.
fn draw_preview(
    frame: &mut Frame,
    area: Rect,
    entry: Option<&Entry>,
    scroll: usize,
    palette: Palette,
) {
    let lines: Vec<Line> = entry
        .map(|e| {
            e.contents
                .lines()
                .map(|raw| Line::from(raw).style(preview_line(raw, palette)))
                .collect()
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(palette.muted))
                    .padding(Padding::left(GUTTER)),
            )
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        area,
    );
}

fn draw(frame: &mut Frame, view: &PickerView) {
    let PickerView {
        visible,
        cursor,
        show_all,
        pending,
        hidden,
        preview_on,
        preview_scroll,
        palette,
    } = *view;
    let area = frame.area();
    let [header, body, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let mut title = String::from(" caleb — open session");
    if show_all {
        title.push_str(" (showing all)");
    }
    frame.render_widget(
        Line::from(title).style(Style::default().add_modifier(Modifier::BOLD)),
        Rect {
            height: 1,
            ..header
        },
    );

    // The list keeps a fixed width so its rows never truncate; the preview
    // takes whatever is left, which is where extra width is worth more.
    let (body, preview) = if preview_on && preview_fits(area.width) {
        let [list, preview] =
            Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Min(0)]).areas(body);
        (list, Some(preview))
    } else {
        (body, None)
    };

    if let Some(area) = preview {
        draw_preview(
            frame,
            area,
            visible.get(cursor).copied(),
            preview_scroll,
            palette,
        );
    }

    if visible.is_empty() {
        let message = if hidden > 0 {
            format!("  no unfinished sessions — press 'a' to show all {hidden}")
        } else {
            "  no sessions found".to_string()
        };
        frame.render_widget(Line::from(message), body);
    } else {
        let items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut style = Style::default();
                if i == cursor {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                ListItem::new(Line::from(row_text(e, i == cursor)).style(style))
            })
            .collect();
        let mut state = ListState::default();
        frame.render_stateful_widget(List::new(items), body, &mut state);
    }

    let _ = palette; // picker chrome is monochrome
    let status_line = if let Some(e) = pending.and_then(|i| visible.get(i)) {
        Line::from(format!(" delete {}?   y/n", pretty_name(&e.name)))
            .style(Style::default().add_modifier(Modifier::BOLD))
    } else {
        {
            // No point advertising a key that does nothing at this width.
            let hint = if preview_fits(area.width) {
                " j/k move   Enter open   d delete   p preview   a show all   Esc cancel"
            } else {
                " j/k move   Enter open   d delete   a show all   Esc cancel"
            };
            Line::from(hint).style(Style::default().add_modifier(Modifier::DIM))
        }
    };
    frame.render_widget(status_line, status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(name: &str, open: u32, total: u32) -> Entry {
        entry_with(name, open, total, "")
    }

    fn entry_with(name: &str, open: u32, total: u32, contents: &str) -> Entry {
        Entry {
            name: name.to_string(),
            open,
            total,
            contents: contents.to_string(),
        }
    }

    #[test]
    fn scan_lists_md_files_newest_first_with_counts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("2026-05-30_10-00.md"),
            "## Active\n- [ ] one\n## Completed\n- [x] two\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("2026-05-31_14-30.md"),
            "## Active\n- [ ] x\n- [ ] y\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("ignore.txt"), "nope").unwrap();

        let entries = scan(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "2026-05-31_14-30.md");
        assert_eq!((entries[0].open, entries[0].total), (2, 2));
        assert_eq!(entries[1].name, "2026-05-30_10-00.md");
        assert_eq!((entries[1].open, entries[1].total), (1, 2));
    }

    #[test]
    fn scan_of_an_empty_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn filter_hides_fully_completed_sessions_by_default() {
        let entries = vec![
            entry("2026-05-31_14-30.md", 1, 3),
            entry("2026-05-30_10-00.md", 0, 5),
        ];
        let visible = filter_visible(&entries, false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "2026-05-31_14-30.md");
    }

    #[test]
    fn filter_show_all_includes_finished_sessions() {
        let entries = vec![
            entry("2026-05-31_14-30.md", 1, 3),
            entry("2026-05-30_10-00.md", 0, 5),
        ];
        assert_eq!(filter_visible(&entries, true).len(), 2);
    }

    #[test]
    fn pretty_name_formats_the_timestamp() {
        assert_eq!(pretty_name("2026-05-31_14-30.md"), "2026-05-31  14:30");
    }

    #[test]
    fn pretty_name_keeps_collision_suffixes() {
        assert_eq!(pretty_name("2026-05-31_14-30-2.md"), "2026-05-31  14:30-2");
    }

    #[test]
    fn pretty_name_passes_through_unrecognized_names() {
        assert_eq!(pretty_name("notes.md"), "notes.md");
    }

    #[test]
    fn pretty_name_does_not_split_a_multibyte_char() {
        // Positions 4/7/10/13 are the expected separators, but the minute
        // field runs into a 2-byte char — slicing at 16 would panic.
        let name = "2026-05-31_14-3é.md";
        assert_eq!(pretty_name(name), name);
    }

    fn view<'a>(visible: &'a [&'a Entry]) -> PickerView<'a> {
        PickerView {
            visible,
            cursor: 0,
            show_all: false,
            pending: None,
            hidden: 0,
            preview_on: true,
            preview_scroll: 0,
            palette: Palette::new(true),
        }
    }

    fn render(width: u16, height: u16, view: &PickerView) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                draw(f, view);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The whole screen as text, for assertions that do not care where a
    /// string landed.
    fn screen(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| row(buf, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn y_confirms_the_delete() {
        assert_eq!(confirm_key(KeyCode::Char('y')), Confirm::Delete);
    }

    #[test]
    fn n_cancels_the_delete() {
        assert_eq!(confirm_key(KeyCode::Char('n')), Confirm::Cancel);
    }

    #[test]
    fn any_unrecognized_key_cancels_rather_than_deleting() {
        // The prompt must never fall through to a delete on a stray key.
        for code in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('d'),
            KeyCode::Char('Y'),
            KeyCode::Down,
        ] {
            assert_eq!(confirm_key(code), Confirm::Cancel, "{code:?} must cancel");
        }
    }

    #[test]
    fn delete_entry_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-05-31_14-30.md");
        std::fs::write(&path, "## Active\n- [ ] x\n").unwrap();

        delete_entry(dir.path(), "2026-05-31_14-30.md").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_entry_leaves_other_sessions_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "## Active\n").unwrap();
        std::fs::write(dir.path().join("b.md"), "## Active\n").unwrap();

        delete_entry(dir.path(), "a.md").unwrap();
        assert!(!dir.path().join("a.md").exists());
        assert!(dir.path().join("b.md").exists());
    }

    #[test]
    fn delete_entry_of_a_missing_file_is_an_error() {
        // run() swallows this and re-scans; the caller still needs to see it.
        let dir = tempfile::tempdir().unwrap();
        assert!(delete_entry(dir.path(), "gone.md").is_err());
    }

    #[test]
    fn the_hint_line_advertises_the_delete_key() {
        let e = entry("2026-05-31_14-30.md", 1, 2);
        let visible = vec![&e];
        let buf = render(70, 6, &view(&visible));
        assert!(
            row(&buf, 5).contains("d delete"),
            "status row was: {:?}",
            row(&buf, 5)
        );
    }

    #[test]
    fn a_pending_delete_replaces_the_hint_line_with_a_confirm_prompt() {
        let e = entry("2026-05-31_14-30.md", 1, 2);
        let visible = vec![&e];
        let buf = render(
            70,
            6,
            &PickerView {
                pending: Some(0),
                ..view(&visible)
            },
        );
        let status = row(&buf, 5);
        assert!(status.contains("delete"), "status row was: {status:?}");
        assert!(
            status.contains("2026-05-31  14:30"),
            "prompt must name the session, got: {status:?}"
        );
        assert!(status.contains("y/n"), "status row was: {status:?}");
    }

    #[test]
    fn opening_a_store_of_only_finished_sessions_shows_them_all() {
        // Otherwise the picker opens onto an empty box with no hint that
        // anything is there.
        let entries = vec![entry("a.md", 0, 3), entry("b.md", 0, 1)];
        assert!(show_all_on_open(&entries));
    }

    #[test]
    fn opening_a_store_with_unfinished_work_does_not_show_all() {
        let entries = vec![entry("a.md", 0, 3), entry("b.md", 2, 4)];
        assert!(!show_all_on_open(&entries));
    }

    #[test]
    fn opening_an_empty_store_does_not_show_all() {
        assert!(!show_all_on_open(&[]));
    }

    #[test]
    fn an_emptied_list_names_the_finished_sessions_still_on_disk() {
        // Deleting the last unfinished session must not silently reveal the
        // finished ones; it says how many are there and how to see them.
        let buf = render(
            70,
            6,
            &PickerView {
                hidden: 4,
                ..view(&[])
            },
        );
        let body = row(&buf, 2);
        assert!(
            body.contains("no unfinished sessions"),
            "body was: {body:?}"
        );
        assert!(
            body.contains('4'),
            "body must count what is hidden: {body:?}"
        );
        assert!(body.contains('a'), "body must name the key: {body:?}");
    }

    #[test]
    fn a_genuinely_empty_store_still_says_no_sessions_found() {
        let buf = render(70, 6, &view(&[]));
        assert!(row(&buf, 2).contains("no sessions found"));
    }

    #[test]
    fn the_preview_fits_at_eighty_columns() {
        assert!(preview_fits(80));
    }

    #[test]
    fn the_preview_is_dropped_below_eighty_columns() {
        assert!(!preview_fits(79));
        assert!(!preview_fits(0));
    }

    #[test]
    fn scrolling_stops_with_the_last_line_on_screen() {
        // 30 lines in a 10-row pane: the furthest useful offset is 20.
        assert_eq!(clamp_scroll(25, 30, 10), 20);
        assert_eq!(clamp_scroll(5, 30, 10), 5);
    }

    #[test]
    fn content_shorter_than_the_pane_never_scrolls() {
        assert_eq!(clamp_scroll(7, 4, 10), 0);
    }

    #[test]
    fn a_pane_taller_than_its_content_does_not_underflow() {
        // `lines - height` wraps around on unsigned math, which would send
        // the view billions of rows past the end instead of to the top.
        assert_eq!(clamp_scroll(3, 4, 10), 0);
        assert_eq!(clamp_scroll(usize::MAX, 1, 400), 0);
    }

    #[test]
    fn clamping_never_raises_the_requested_offset() {
        assert_eq!(clamp_scroll(0, 500, 10), 0);
        assert_eq!(clamp_scroll(3, 500, 0), 3);
    }

    #[test]
    fn scan_keeps_the_file_contents_for_the_preview() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("2026-05-31_14-30.md"),
            "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] the only task\n",
        )
        .unwrap();
        let entries = scan(dir.path()).unwrap();
        assert!(
            entries[0].contents.contains("- [ ] the only task"),
            "scan already reads the file; it must keep what it read"
        );
    }

    const SAMPLE: &str = "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] wire up the parser\n\n## Completed\n\n- [x] read the chapter\n";

    /// Column and row where `needle` starts. `str::find` gives a byte offset,
    /// and rows carry multi-byte glyphs (`▸`, `│`), so the count of characters
    /// before the match is what lines up with a cell coordinate.
    fn find(buf: &ratatui::buffer::Buffer, needle: &str) -> (u16, u16) {
        for y in 0..buf.area.height {
            let line = row(buf, y);
            if let Some(byte) = line.find(needle) {
                return (line[..byte].chars().count() as u16, y);
            }
        }
        panic!("{needle:?} is not on screen:\n{}", screen(buf));
    }

    #[test]
    fn the_preview_shows_the_highlighted_sessions_contents() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let text = screen(&buf);
        assert!(
            text.contains("wire up the parser"),
            "preview missing:\n{text}"
        );
        assert!(
            text.contains("read the chapter"),
            "preview missing:\n{text}"
        );
    }

    #[test]
    fn the_preview_is_dropped_on_a_narrow_terminal() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(79, 12, &view(&visible));
        let text = screen(&buf);
        assert!(
            !text.contains("wire up the parser"),
            "narrow terminals get no preview:\n{text}"
        );
        assert!(
            text.contains("2026-05-31  14:30"),
            "the list must remain:\n{text}"
        );
    }

    #[test]
    fn the_preview_can_be_toggled_off_on_a_wide_terminal() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(
            100,
            12,
            &PickerView {
                preview_on: false,
                ..view(&visible)
            },
        );
        assert!(!screen(&buf).contains("wire up the parser"));
    }

    #[test]
    fn the_preview_follows_the_cursor() {
        let a = entry_with("2026-05-31_14-30.md", 1, 1, "- [ ] the newer one\n");
        let b = entry_with("2026-05-30_10-00.md", 1, 1, "- [ ] the older one\n");
        let visible = vec![&a, &b];
        let buf = render(
            100,
            12,
            &PickerView {
                cursor: 1,
                ..view(&visible)
            },
        );
        let text = screen(&buf);
        assert!(
            text.contains("the older one"),
            "preview should track row 1:\n{text}"
        );
        assert!(
            !text.contains("the newer one"),
            "row 0 should not show:\n{text}"
        );
    }

    #[test]
    fn the_preview_scrolls_past_earlier_lines() {
        let mut long = String::new();
        for i in 0..40 {
            use std::fmt::Write;
            writeln!(long, "- [ ] task number {i}").unwrap();
        }
        let e = entry_with("2026-05-31_14-30.md", 40, 40, &long);
        let visible = vec![&e];
        let buf = render(
            100,
            12,
            &PickerView {
                preview_scroll: 20,
                ..view(&visible)
            },
        );
        let text = screen(&buf);
        assert!(
            text.contains("task number 20"),
            "should be scrolled to 20:\n{text}"
        );
        assert!(
            !text.contains("task number 0\n"),
            "line 0 should be above the fold"
        );
    }

    #[test]
    fn the_session_heading_is_bold_in_the_preview() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let (x, y) = find(&buf, "# Session");
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn completed_tasks_are_struck_through_in_the_preview() {
        // Matches how the session screen renders a done task.
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let (x, y) = find(&buf, "- [x] read the chapter");
        assert!(buf[(x, y)].modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn open_tasks_are_not_struck_through_in_the_preview() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let (x, y) = find(&buf, "- [ ] wire up the parser");
        assert!(!buf[(x, y)].modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn the_hint_line_advertises_the_preview_toggle() {
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        assert!(
            row(&buf, 11).contains("p preview"),
            "status: {:?}",
            row(&buf, 11)
        );
    }

    #[test]
    fn preview_headings_take_their_palette_colors() {
        // The picker chrome is monochrome, but the preview is content and
        // earns the same palette the session screen uses.
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let palette = Palette::new(true);

        let (x, y) = find(&buf, "# Session");
        assert_eq!(buf[(x, y)].fg, palette.accent, "session heading");

        let (x, y) = find(&buf, "## Active");
        assert_eq!(buf[(x, y)].fg, palette.help, "pane heading");
    }

    #[test]
    fn lines_that_fit_are_counted_once_each() {
        assert_eq!(wrapped_lines("one\ntwo\nthree\n", 40), 3);
    }

    #[test]
    fn a_blank_line_still_occupies_a_row() {
        assert_eq!(wrapped_lines("a\n\nb\n", 40), 3);
    }

    #[test]
    fn an_overlong_line_occupies_the_rows_it_wraps_onto() {
        // 150 bytes is the task cap; at 30 columns that is five rows.
        let long = format!("- [ ] {}", "x".repeat(144));
        assert_eq!(wrapped_lines(&long, 30), 5);
    }

    #[test]
    fn empty_contents_have_no_height() {
        assert_eq!(wrapped_lines("", 40), 0);
    }

    #[test]
    fn a_zero_width_pane_does_not_divide_by_zero() {
        assert_eq!(wrapped_lines("abc\n", 0), 1);
    }

    #[test]
    fn the_preview_text_clears_the_border() {
        // Text hard against the divider reads as though it belongs to it.
        let e = entry_with("2026-05-31_14-30.md", 1, 2, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let (x, _) = find(&buf, "# Session");
        assert_eq!(
            x,
            LIST_WIDTH + 1 + GUTTER,
            "list width, then the border, then the padding"
        );
    }

    #[test]
    fn a_row_right_aligns_its_counts() {
        let e = entry("2026-05-31_14-30.md", 2, 3);
        let text = row_text(&e, false);
        assert_eq!(
            text.chars().count(),
            ROW_WIDTH as usize,
            "the row should span the pane less its gutter: {text:?}"
        );
        assert!(text.ends_with("2 open / 3 total"), "{text:?}");
        assert!(text.starts_with("  2026-05-31  14:30"), "{text:?}");
    }

    #[test]
    fn wider_counts_stay_flush_with_narrower_ones() {
        // The whole point: variable-width counts still end on the same column.
        let narrow = row_text(&entry("2026-05-31_14-30.md", 2, 3), false);
        let wide = row_text(&entry("2026-05-30_10-00.md", 12, 137), false);
        assert_eq!(narrow.chars().count(), wide.chars().count());
    }

    #[test]
    fn the_selected_row_carries_the_marker() {
        let e = entry("2026-05-31_14-30.md", 2, 3);
        assert!(row_text(&e, true).starts_with("\u{25b8} "));
        assert!(row_text(&e, false).starts_with("  "));
    }

    #[test]
    fn a_row_too_wide_to_align_keeps_a_space_between_its_halves() {
        // A hand-named file can be longer than the pane; the name and the
        // counts must not run together into one unreadable string.
        let e = entry("a-very-long-hand-written-session-name-indeed.md", 12, 137);
        let text = row_text(&e, false);
        assert!(
            text.contains(" 12 open / 137 total"),
            "halves should stay separated: {text:?}"
        );
    }

    #[test]
    fn the_counts_stop_a_gutter_short_of_the_divider() {
        let e = entry_with("2026-05-31_14-30.md", 2, 3, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));
        let (x, _) = find(&buf, "2 open / 3 total");
        let end = x as usize + "2 open / 3 total".chars().count();
        assert_eq!(
            end,
            (LIST_WIDTH - GUTTER) as usize,
            "counts should stop a gutter short of the border at {LIST_WIDTH}"
        );
    }

    #[test]
    fn the_gutters_on_both_sides_of_the_divider_match() {
        // Measured off the rendered screen, naming no constant: this is what
        // catches one side being widened without the other.
        let e = entry_with("2026-05-31_14-30.md", 2, 3, SAMPLE);
        let visible = vec![&e];
        let buf = render(100, 12, &view(&visible));

        let (divider, _) = find(&buf, "\u{2502}");
        let (counts, _) = find(&buf, "2 open / 3 total");
        let counts_end = counts + "2 open / 3 total".chars().count() as u16;
        let (text, _) = find(&buf, "# Session");

        assert_eq!(
            divider - counts_end,
            text - divider - 1,
            "left gutter {} vs right gutter {}",
            divider - counts_end,
            text - divider - 1
        );
    }
}
