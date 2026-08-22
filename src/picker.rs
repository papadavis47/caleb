//! Resume picker: one screen to choose a past session.

use crate::markdown::count_tasks;
use crate::storage::FILE_EXTENSION;
use crate::tui::Tui;
use crate::ui::{ClickTracker, Palette};
use crossterm::event::{self, Event, KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState};
use std::path::Path;
use std::time::Instant;

/// First screen row occupied by a session entry; the 2-row header sits above.
const FIRST_ROW: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub open: u32,
    pub total: u32,
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
                &visible,
                cursor,
                show_all,
                pending,
                entries.len() - visible.len(),
                palette,
            );
        })?;

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
                        KeyCode::Char('d') => {
                            if cursor < visible.len() {
                                pending = Some(cursor);
                            }
                        }
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

        if let Some(name) = confirmed {
            // The cursor is deliberately left where it is: the next session
            // slides up under it. A failed delete is swallowed because the
            // re-scan reports the truth either way.
            let _ = delete_entry(dir, &name);
            entries = scan(dir)?;
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

fn draw(
    frame: &mut Frame,
    visible: &[&Entry],
    cursor: usize,
    show_all: bool,
    pending: Option<usize>,
    hidden: usize,
    palette: Palette,
) {
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
                let marker = if i == cursor { "▸ " } else { "  " };
                ListItem::new(
                    Line::from(format!(
                        "{marker}{}   {} open / {} total",
                        pretty_name(&e.name),
                        e.open,
                        e.total
                    ))
                    .style(style),
                )
            })
            .collect();
        let mut state = ListState::default();
        frame.render_stateful_widget(List::new(items), body, &mut state);
    }

    let _ = palette; // picker chrome is monochrome
    let status_line = match pending.and_then(|i| visible.get(i)) {
        Some(e) => Line::from(format!(" delete {}?   y/n", pretty_name(&e.name)))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        None => Line::from(" j/k move   Enter open   d delete   a show all   Esc cancel")
            .style(Style::default().add_modifier(Modifier::DIM)),
    };
    frame.render_widget(status_line, status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(name: &str, open: u32, total: u32) -> Entry {
        Entry {
            name: name.to_string(),
            open,
            total,
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

    fn render(
        width: u16,
        height: u16,
        visible: &[&Entry],
        cursor: usize,
        pending: Option<usize>,
        hidden: usize,
    ) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                draw(
                    f,
                    visible,
                    cursor,
                    false,
                    pending,
                    hidden,
                    Palette::new(false),
                );
            })
            .unwrap();
        terminal.backend().buffer().clone()
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
        let buf = render(70, 6, &visible, 0, None, 0);
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
        let buf = render(70, 6, &visible, 0, Some(0), 0);
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
        let buf = render(70, 6, &[], 0, None, 4);
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
        let buf = render(70, 6, &[], 0, None, 0);
        assert!(row(&buf, 2).contains("no sessions found"));
    }
}
