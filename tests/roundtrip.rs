//! End-to-end persistence through the public API.
//!
//! These complement the unit tests rather than repeat them: each one crosses a
//! module boundary (session → markdown → storage → disk) that no single
//! module's `mod tests` can reach on its own.

use caleb::markdown;
use caleb::model::{Pane, Timestamp};
use caleb::session::{self, Session};

fn ts(hour: u8, minute: u8) -> Timestamp {
    Timestamp {
        year: 2026,
        month: 5,
        day: 31,
        hour,
        minute,
    }
}

#[test]
fn a_session_survives_save_load_and_resume() {
    let dir = tempfile::tempdir().unwrap();

    let mut s = session::create_new(dir.path(), ts(14, 30)).unwrap();
    s.add(Pane::Active, "write the parser");
    s.add(Pane::Active, "write the renderer");
    s.add(Pane::Completed, "read the spec");
    s.save(dir.path()).unwrap();

    let reloaded = Session::load(dir.path(), &s.filename).unwrap();
    assert_eq!(reloaded.active.len(), 2);
    assert_eq!(reloaded.completed.len(), 1);

    let resumed = session::resume(dir.path(), &s.filename, ts(16, 45)).unwrap();
    assert_eq!(resumed.filename, "2026-05-31_16-45.md");
    assert_eq!(resumed.timestamp, Some(ts(16, 45)));
    assert_eq!(resumed.active[0].text, "write the parser");
    assert_eq!(resumed.completed[0].text, "read the spec");
    assert!(
        !dir.path().join(&s.filename).exists(),
        "resuming consumes the old file"
    );

    // Filename and header agree immediately, with no edit required.
    let on_disk = std::fs::read_to_string(dir.path().join(&resumed.filename)).unwrap();
    assert!(on_disk.starts_with("# Session 2026-05-31 16:45\n"));
    assert_eq!(
        Session::load(dir.path(), &resumed.filename)
            .unwrap()
            .timestamp,
        Some(ts(16, 45))
    );
}

#[test]
fn the_on_disk_format_is_stable_across_a_save_load_save_cycle() {
    let dir = tempfile::tempdir().unwrap();

    let mut s = session::create_new(dir.path(), ts(9, 5)).unwrap();
    s.add(Pane::Active, "still open");
    s.add(Pane::Completed, "already done");
    s.save(dir.path()).unwrap();
    let first = std::fs::read_to_string(dir.path().join(&s.filename)).unwrap();

    let mut reloaded = Session::load(dir.path(), &s.filename).unwrap();
    reloaded.save(dir.path()).unwrap();
    let second = std::fs::read_to_string(dir.path().join(&s.filename)).unwrap();

    assert_eq!(first, second, "a load/save cycle must not perturb the file");
}

#[test]
fn a_hand_written_file_parses_into_the_expected_panes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("2026-05-31_14-30.md");
    std::fs::write(
        &path,
        "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] one\n\n## Completed\n\n- [x] two\n",
    )
    .unwrap();

    let s = Session::load(dir.path(), "2026-05-31_14-30.md").unwrap();
    assert_eq!(s.timestamp, Some(ts(14, 30)));
    assert_eq!(s.active[0].text, "one");
    assert_eq!(s.completed[0].text, "two");
    assert!(!s.dirty, "a freshly loaded session is not dirty");
}

#[test]
fn the_picker_still_counts_a_file_that_is_too_malformed_to_open() {
    // A session must never disappear from the resume list just because it
    // cannot be parsed — the user needs to see it in order to repair it.
    let dir = tempfile::tempdir().unwrap();
    let long = "x".repeat(caleb::model::MAX_TASK_BYTES + 1);
    std::fs::write(
        dir.path().join("2026-05-31_14-30.md"),
        format!("# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] {long}\n- [x] fine\n"),
    )
    .unwrap();

    assert!(Session::load(dir.path(), "2026-05-31_14-30.md").is_err());

    let entries = caleb::picker::scan(dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].open, 1);
    assert_eq!(entries[0].total, 2);

    let counts = markdown::count_tasks(
        &std::fs::read_to_string(dir.path().join("2026-05-31_14-30.md")).unwrap(),
    );
    assert_eq!(counts.total, 2);
}

#[test]
fn colliding_timestamps_get_distinct_files() {
    let dir = tempfile::tempdir().unwrap();

    let mut a = session::create_new(dir.path(), ts(14, 30)).unwrap();
    a.add(Pane::Active, "first session");
    a.save(dir.path()).unwrap();

    let mut b = session::create_new(dir.path(), ts(14, 30)).unwrap();
    b.add(Pane::Active, "second session");
    b.save(dir.path()).unwrap();

    assert_ne!(a.filename, b.filename);
    assert_eq!(b.filename, "2026-05-31_14-30-2.md");
    assert_eq!(
        Session::load(dir.path(), &a.filename).unwrap().active[0].text,
        "first session"
    );
    assert_eq!(
        Session::load(dir.path(), &b.filename).unwrap().active[0].text,
        "second session"
    );
}

#[test]
fn pulled_tasks_cross_the_file_boundary_and_are_not_offered_twice() {
    let dir = tempfile::tempdir().unwrap();

    let mut old = session::create_new(dir.path(), ts(14, 30)).unwrap();
    old.add(Pane::Active, "carry me");
    old.add(Pane::Active, "and me");
    old.add(Pane::Active, "not me");
    old.save(dir.path()).unwrap();

    let mut current = session::create_new(dir.path(), ts(16, 45)).unwrap();
    let moved = session::pull_from_file(
        dir.path(),
        &old.filename,
        &mut current,
        &[(0, "carry me".to_string()), (1, "and me".to_string())],
    )
    .unwrap();
    assert_eq!(moved, 2);

    // Both files, re-read from disk through the public API.
    let source = Session::load(dir.path(), &old.filename).unwrap();
    assert_eq!(source.active.len(), 1);
    assert_eq!(source.active[0].text, "not me");
    let done: Vec<&str> = source.completed.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(done, ["carry me", "and me"]);

    let target = Session::load(dir.path(), &current.filename).unwrap();
    let carried: Vec<&str> = target.active.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(carried, ["carry me", "and me"]);
    assert!(target.active.iter().all(|t| !t.done));

    // A second trip through the picker offers only what stayed behind.
    let entries = caleb::picker::scan(dir.path()).unwrap();
    let offered = caleb::pull::candidates(&entries, &current.filename);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].name, old.filename);
    assert_eq!(offered[0].open, vec![(0, "not me".to_string())]);
}
