//! Parse and serialize the GitHub-flavored task-list format.
//!
//! Pure functions: bytes in, typed data out. No I/O, no globals.

use crate::model::{MAX_TASK_BYTES, Task, Timestamp};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("task text exceeds {MAX_TASK_BYTES} bytes")]
    LineTooLong,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub timestamp: Option<Timestamp>,
    pub active: Vec<Task>,
    pub completed: Vec<Task>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Active,
    Completed,
}

pub fn parse(source: &str) -> Result<Parsed, ParseError> {
    let mut result = Parsed::default();
    let mut section = Section::None;

    for raw in source.split('\n') {
        // Tolerate CRLF files.
        let line = raw.strip_suffix('\r').unwrap_or(raw);

        if let Some(ts) = parse_header(line) {
            result.timestamp = Some(ts);
            continue;
        }
        if line == "## Active" {
            section = Section::Active;
            continue;
        }
        if line == "## Completed" {
            section = Section::Completed;
            continue;
        }
        if let Some((text, done)) = parse_task_line(line) {
            if text.len() > MAX_TASK_BYTES {
                return Err(ParseError::LineTooLong);
            }
            let task = Task {
                text: text.to_string(),
                done,
            };
            // Tasks before any heading default to active — easier to
            // recover from a hand-edited file than rejecting it.
            match section {
                Section::Completed => result.completed.push(task),
                Section::None | Section::Active => result.active.push(task),
            }
        }
        // Anything else: silently ignored.
    }

    Ok(result)
}

/// Expected exact shape: `# Session YYYY-MM-DD HH:MM`.
///
/// Rust note: this returns `Option`, not `Result` — a bad header is not an
/// error, it just means "no timestamp". The `?` operator on `Option` makes
/// each failed check bail out to `None` with no nesting.
fn parse_header(line: &str) -> Option<Timestamp> {
    let rest = line.strip_prefix("# Session ")?;
    if rest.len() != 16 {
        return None;
    }
    let b = rest.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b' ' || b[13] != b':' {
        return None;
    }
    let ts = Timestamp {
        year: rest[0..4].parse().ok()?,
        month: rest[5..7].parse().ok()?,
        day: rest[8..10].parse().ok()?,
        hour: rest[11..13].parse().ok()?,
        minute: rest[14..16].parse().ok()?,
    };
    // Light sanity bounds — guards against lines like "13:99".
    if !(1..=12).contains(&ts.month)
        || !(1..=31).contains(&ts.day)
        || ts.hour > 23
        || ts.minute > 59
    {
        return None;
    }
    Some(ts)
}

/// Open and total task counts for a session file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskCounts {
    pub open: u32,
    pub total: u32,
}

/// Count tasks without fully parsing, for the resume picker's summary column.
///
/// Deliberately more tolerant than [`parse`]: a hand-edited file with an
/// over-long task still counts, where `parse` would reject the whole file.
/// A session must never vanish from the picker just because it cannot be
/// opened — the user needs to see it in order to fix it.
///
/// ```
/// # use caleb::markdown::count_tasks;
/// let counts = count_tasks("- [ ] todo\n- [x] done\n");
/// assert_eq!((counts.open, counts.total), (1, 2));
/// ```
pub fn count_tasks(source: &str) -> TaskCounts {
    let mut counts = TaskCounts { open: 0, total: 0 };
    for raw in source.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some((_, done)) = parse_task_line(line) {
            counts.total += 1;
            if !done {
                counts.open += 1;
            }
        }
    }
    counts
}

fn parse_task_line(line: &str) -> Option<(&str, bool)> {
    if let Some(text) = line.strip_prefix("- [ ] ") {
        return Some((text, false));
    }
    if let Some(text) = line.strip_prefix("- [x] ") {
        return Some((text, true));
    }
    None
}

pub fn serialize(timestamp: Option<Timestamp>, active: &[Task], completed: &[Task]) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    match timestamp {
        Some(ts) => {
            let _ = writeln!(out, "# Session {ts}\n");
        }
        None => out.push_str("# Session\n\n"),
    }

    out.push_str("## Active\n\n");
    for t in active {
        let _ = writeln!(out, "- [ ] {}", t.text);
    }
    out.push_str("\n## Completed\n\n");
    for t in completed {
        let _ = writeln!(out, "- [x] {}", t.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn count_tasks_tolerates_lines_that_parse_would_reject() {
        // The picker must still list a session whose file has a hand-edited
        // over-long task, even though `parse` refuses to open it.
        let src = format!("- [ ] {}\n- [x] ok\n", "x".repeat(MAX_TASK_BYTES + 1));
        assert!(parse(&src).is_err());
        let c = count_tasks(&src);
        assert_eq!(c.open, 1);
        assert_eq!(c.total, 2);
    }

    fn task(text: &str, done: bool) -> Task {
        Task {
            text: text.to_string(),
            done,
        }
    }

    #[test]
    fn parse_empty_yields_nothing() {
        let p = parse("").unwrap();
        assert_eq!(p.timestamp, None);
        assert!(p.active.is_empty());
        assert!(p.completed.is_empty());
    }

    #[test]
    fn parse_header_sets_timestamp() {
        let p = parse("# Session 2026-05-31 14:30\n").unwrap();
        assert_eq!(
            p.timestamp,
            Some(Timestamp {
                year: 2026,
                month: 5,
                day: 31,
                hour: 14,
                minute: 30
            })
        );
    }

    #[test]
    fn parse_malformed_header_is_ignored() {
        assert_eq!(parse("# Session not-a-date\n").unwrap().timestamp, None);
    }

    #[test]
    fn parse_out_of_range_header_is_ignored() {
        assert_eq!(
            parse("# Session 2026-13-99 25:61\n").unwrap().timestamp,
            None
        );
    }

    #[test]
    fn parse_classifies_tasks_by_section() {
        let src = "## Active\n- [ ] alpha\n- [ ] beta\n## Completed\n- [x] gamma\n";
        let p = parse(src).unwrap();
        assert_eq!(p.active, vec![task("alpha", false), task("beta", false)]);
        assert_eq!(p.completed, vec![task("gamma", true)]);
    }

    #[test]
    fn parse_orphan_tasks_default_to_active() {
        let p = parse("- [ ] orphan\n").unwrap();
        assert_eq!(p.active, vec![task("orphan", false)]);
    }

    #[test]
    fn parse_ignores_unknown_lines() {
        let src = "## Active\nsome prose\n- [ ] real task\n> quote\n- not a task\n";
        let p = parse(src).unwrap();
        assert_eq!(p.active, vec![task("real task", false)]);
    }

    #[test]
    fn parse_rejects_oversize_task() {
        let src = format!("- [ ] {}\n", "x".repeat(MAX_TASK_BYTES + 1));
        assert_eq!(parse(&src), Err(ParseError::LineTooLong));
    }

    #[test]
    fn parse_tolerates_crlf() {
        let src = "# Session 2026-05-31 14:30\r\n## Active\r\n- [ ] windows-style\r\n";
        let p = parse(src).unwrap();
        assert_eq!(p.active, vec![task("windows-style", false)]);
        assert_eq!(p.timestamp.unwrap().minute, 30);
    }

    #[test]
    fn serialize_canonical_shape() {
        let out = serialize(
            Some(Timestamp {
                year: 2026,
                month: 5,
                day: 31,
                hour: 14,
                minute: 30,
            }),
            &[task("first", false), task("second", false)],
            &[task("third", true)],
        );
        assert_eq!(
            out,
            "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] first\n- [ ] second\n\n## Completed\n\n- [x] third\n"
        );
    }

    #[test]
    fn serialize_omits_date_when_none() {
        assert_eq!(
            serialize(None, &[], &[]),
            "# Session\n\n## Active\n\n\n## Completed\n\n"
        );
    }

    #[test]
    fn round_trip_is_byte_stable() {
        let original = "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] alpha\n- [ ] beta\n\n## Completed\n\n- [x] gamma\n";
        let first = parse(original).unwrap();
        let out = serialize(first.timestamp, &first.active, &first.completed);
        assert_eq!(out, original);
        assert_eq!(parse(&out).unwrap(), first);
    }
}
