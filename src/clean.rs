//! `--clean`: drop session files that have nothing left to do.
//!
//! Split the same way `storage` is: [`cleanable`] is a pure decision over
//! already-scanned entries, and [`delete`] is the one function that touches
//! the filesystem, so the rule can be tested without a tempdir.

use crate::picker::Entry;
use std::path::Path;

/// Sessions with no open tasks. A file with no tasks at all counts — an
/// empty session has nothing left to do either.
pub fn cleanable(entries: &[Entry]) -> Vec<&Entry> {
    entries.iter().filter(|e| e.open == 0).collect()
}

/// Delete `names` from `dir`. Returns the names that could not be removed,
/// paired with the reason; a failure on one file does not stop the rest.
pub fn delete(dir: &Path, names: &[&str]) -> Vec<(String, std::io::Error)> {
    let mut failures = Vec::new();
    for name in names {
        if let Err(e) = std::fs::remove_file(dir.join(name)) {
            failures.push(((*name).to_string(), e));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, open: u32, total: u32) -> Entry {
        Entry {
            name: name.to_string(),
            open,
            total,
            contents: String::new(),
        }
    }

    #[test]
    fn keeps_sessions_with_open_tasks() {
        let entries = vec![entry("a.md", 1, 3)];
        assert!(cleanable(&entries).is_empty());
    }

    #[test]
    fn selects_fully_completed_sessions() {
        let entries = vec![entry("a.md", 0, 3)];
        let got = cleanable(&entries);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "a.md");
    }

    #[test]
    fn selects_sessions_with_no_tasks_at_all() {
        let entries = vec![entry("empty.md", 0, 0)];
        assert_eq!(cleanable(&entries).len(), 1);
    }

    #[test]
    fn preserves_scan_order() {
        let entries = vec![
            entry("c.md", 0, 1),
            entry("b.md", 2, 2),
            entry("a.md", 0, 0),
        ];
        let names: Vec<_> = cleanable(&entries).iter().map(|e| &e.name).collect();
        assert_eq!(names, ["c.md", "a.md"]);
    }

    #[test]
    fn delete_removes_only_the_named_files() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["a.md", "b.md"] {
            std::fs::write(dir.path().join(n), "").unwrap();
        }

        let failures = delete(dir.path(), &["a.md"]);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(!dir.path().join("a.md").exists());
        assert!(dir.path().join("b.md").exists());
    }

    #[test]
    fn delete_reports_failures_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.md"), "").unwrap();

        let failures = delete(dir.path(), &["missing.md", "b.md"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "missing.md");
        assert!(!dir.path().join("b.md").exists(), "the rest still go");
    }
}
