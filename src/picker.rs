//! Resume picker: one screen to choose a past session.

use crate::tui::Tui;
use crate::ui::Palette;
use crossterm::event::{self, Event, KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState};
use std::path::Path;
use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// First screen row occupied by a session entry; the 2-row header sits above.
const FIRST_ROW: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub open: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub open: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Selected(String),
    Cancelled,
}

pub fn count_tasks(source: &str) -> Counts {
    let mut counts = Counts { open: 0, total: 0 };
    for raw in source.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("- [ ] ") {
            counts.open += 1;
            counts.total += 1;
        } else if line.starts_with("- [x] ") {
            counts.total += 1;
        }
    }
    counts
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
        if !name.ends_with(".md") {
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

/// `2026-05-31_14-30.md` becomes `2026-05-31  14:30`. Anything that does
/// not match the format prints verbatim.
///
/// Rust note: the separator checks pin positions 4/7/10/13 to ASCII, but not
/// 16 — a name whose minute field runs into a multi-byte character would
/// make `&stem[14..16]` slice mid-sequence and panic. `is_char_boundary`
/// turns that crash into a pass-through.
pub fn pretty_name(name: &str) -> String {
    let stem = name.strip_suffix(".md").unwrap_or(name);
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

pub fn run(dir: &Path, tui: &mut Tui, palette: Palette) -> anyhow::Result<Choice> {
    let entries = scan(dir)?;
    let mut cursor = 0usize;
    let mut show_all = false;
    let mut last_click: Option<(Instant, usize)> = None;

    loop {
        let visible = filter_visible(&entries, show_all);
        // No open sessions — show everything rather than an empty box.
        if visible.is_empty() && !show_all && !entries.is_empty() {
            show_all = true;
            continue;
        }
        if cursor >= visible.len() && !visible.is_empty() {
            cursor = visible.len() - 1;
        }

        tui.terminal()
            .draw(|frame| draw(frame, &visible, cursor, show_all, palette))?;

        match event::read()? {
            Event::Key(key) if key.kind == event::KeyEventKind::Press => match key.code {
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
                KeyCode::Enter => {
                    if let Some(e) = visible.get(cursor) {
                        return Ok(Choice::Selected(e.name.clone()));
                    }
                }
                _ => {}
            },
            Event::Mouse(m) => {
                if let Some(choice) =
                    handle_mouse(m, &visible, &mut cursor, &mut last_click, Instant::now())
                {
                    return Ok(choice);
                }
            }
            _ => {}
        }
    }
}

fn handle_mouse(
    m: MouseEvent,
    visible: &[&Entry],
    cursor: &mut usize,
    last_click: &mut Option<(Instant, usize)>,
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

    let is_double = last_click.is_some_and(|(t, i)| i == index && now - t <= DOUBLE_CLICK);
    *cursor = index;

    if is_double {
        *last_click = None;
        Some(Choice::Selected(entry.name.clone()))
    } else {
        *last_click = Some((now, index));
        None
    }
}

fn draw(frame: &mut Frame, visible: &[&Entry], cursor: usize, show_all: bool, palette: Palette) {
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
        frame.render_widget(Line::from("  no sessions found"), body);
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
    frame.render_widget(
        Line::from(" j/k move   Enter open   a show all   Esc cancel")
            .style(Style::default().add_modifier(Modifier::DIM)),
        status,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, open: u32, total: u32) -> Entry {
        Entry {
            name: name.to_string(),
            open,
            total,
        }
    }

    #[test]
    fn count_tasks_counts_open_and_total() {
        let src = "# Session 2026-05-31 14:30\n## Active\n- [ ] a\n- [ ] b\n## Completed\n- [x] c\n- [x] d\n- [x] e\n";
        let c = count_tasks(src);
        assert_eq!(c.open, 2);
        assert_eq!(c.total, 5);
    }

    #[test]
    fn count_tasks_ignores_prose() {
        let c = count_tasks("just some text\n- not a task\n");
        assert_eq!(c.open, 0);
        assert_eq!(c.total, 0);
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
}
