//! Behavior of the binary itself — argument handling and the non-TTY paths.
//!
//! `cargo test` runs with stdout captured, so every invocation here is
//! non-interactive by construction. That is exactly the surface worth pinning:
//! the TUI paths need a pty (see `scripts/smoke.py`), but these do not, and
//! they are the ones a user hits by piping or scripting caleb.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;

fn caleb(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("caleb").unwrap();
    cmd.env("XDG_DATA_HOME", dir);
    cmd
}

#[test]
fn list_works_without_a_terminal() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("caleb");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("2026-05-31_14-30.md"),
        "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] open one\n\n## Completed\n\n- [x] done\n",
    )
    .unwrap();

    caleb(home.path())
        .arg("--list")
        .assert()
        .success()
        .stdout(predicates::str::contains("2026-05-31_14-30.md"))
        .stdout(predicates::str::contains("1 open / 2 total"));
}

#[test]
fn list_on_an_empty_store_succeeds_silently() {
    let home = tempfile::tempdir().unwrap();
    caleb(home.path())
        .arg("--list")
        .assert()
        .success()
        .stdout("");
}

#[test]
fn list_creates_the_storage_dir_if_it_is_missing() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("caleb");
    assert!(!sessions.exists());

    caleb(home.path()).arg("--list").assert().success();
    assert!(sessions.is_dir(), "the storage dir is created on demand");
}

#[test]
fn launching_the_tui_without_a_terminal_fails_with_a_clear_message() {
    let home = tempfile::tempdir().unwrap();
    caleb(home.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("not a terminal"));
}

#[test]
fn version_and_help_short_circuit() {
    let home = tempfile::tempdir().unwrap();
    caleb(home.path())
        .arg("-v")
        .assert()
        .success()
        .stdout(predicates::str::contains("caleb"));

    caleb(home.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--resume"))
        .stdout(predicates::str::contains("--list"));
}

#[test]
fn help_leads_with_name_version_and_description() {
    let home = tempfile::tempdir().unwrap();
    let out = caleb(home.path()).arg("-h").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines();

    assert_eq!(
        lines.next().unwrap(),
        format!("caleb {}", env!("CARGO_PKG_VERSION")),
        "first line should be the name and version"
    );
    assert_eq!(
        lines.next().unwrap(),
        env!("CARGO_PKG_DESCRIPTION"),
        "second line should be the Cargo.toml description"
    );
}

#[test]
fn help_ends_with_the_repository_url() {
    let home = tempfile::tempdir().unwrap();
    let out = caleb(home.path()).arg("-h").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();

    assert_eq!(
        text.trim_end().lines().last().unwrap(),
        format!("Repository: {}", env!("CARGO_PKG_REPOSITORY")),
        "last line should be the repository URL"
    );
}

#[test]
fn short_and_long_help_agree() {
    let home = tempfile::tempdir().unwrap();
    let short = caleb(home.path()).arg("-h").output().unwrap().stdout;
    let long = caleb(home.path()).arg("--help").output().unwrap().stdout;
    assert_eq!(short, long);
}

#[test]
fn an_unknown_flag_is_rejected() {
    let home = tempfile::tempdir().unwrap();
    caleb(home.path()).arg("--nope").assert().failure();
}

#[test]
fn no_writable_storage_location_is_a_clear_error() {
    // Neither $XDG_DATA_HOME nor $HOME set: caleb cannot know where to look.
    Command::cargo_bin("caleb")
        .unwrap()
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .arg("--list")
        .assert()
        .failure()
        .stderr(predicates::str::contains("XDG_DATA_HOME"));
}

/// A store with one finished session, one still open, and one empty file.
fn store_with_mixed_sessions() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("caleb");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("2026-05-31_14-30.md"),
        "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] open one\n",
    )
    .unwrap();
    std::fs::write(
        sessions.join("2026-05-30_09-00.md"),
        "# Session 2026-05-30 09:00\n\n## Completed\n\n- [x] done\n",
    )
    .unwrap();
    std::fs::write(
        sessions.join("2026-05-29_08-00.md"),
        "# Session 2026-05-29 08:00\n",
    )
    .unwrap();
    home
}

#[test]
fn clean_deletes_only_sessions_without_open_tasks() {
    let home = store_with_mixed_sessions();
    let sessions = home.path().join("caleb");

    caleb(home.path())
        .arg("--clean")
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicates::str::contains("Deleted 2 sessions"));

    assert!(sessions.join("2026-05-31_14-30.md").exists());
    assert!(!sessions.join("2026-05-30_09-00.md").exists());
    assert!(!sessions.join("2026-05-29_08-00.md").exists());
}

#[test]
fn clean_lists_the_sessions_before_asking() {
    let home = store_with_mixed_sessions();
    caleb(home.path())
        .arg("--clean")
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(predicates::str::contains("2026-05-30_09-00.md"))
        .stdout(predicates::str::contains("2026-05-29_08-00.md"))
        .stdout(predicates::str::contains("Delete 2 sessions? [y/N]"))
        .stdout(predicates::str::contains("2026-05-31_14-30.md").not());
}

#[test]
fn clean_deletes_nothing_when_the_answer_is_not_yes() {
    let home = store_with_mixed_sessions();
    let sessions = home.path().join("caleb");

    for answer in ["n\n", "\n", "yep\n", ""] {
        caleb(home.path())
            .arg("--clean")
            .write_stdin(answer)
            .assert()
            .success()
            .stdout(predicates::str::contains("Cancelled"));

        assert_eq!(
            std::fs::read_dir(&sessions).unwrap().count(),
            3,
            "answer {answer:?} should leave every session in place"
        );
    }
}

#[test]
fn clean_says_so_when_there_is_nothing_to_clean() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("caleb");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("2026-05-31_14-30.md"),
        "# Session 2026-05-31 14:30\n\n## Active\n\n- [ ] open one\n",
    )
    .unwrap();

    caleb(home.path())
        .arg("--clean")
        .assert()
        .success()
        .stdout(predicates::str::contains("No sessions to clean"));
    assert!(sessions.join("2026-05-31_14-30.md").exists());
}

#[test]
fn clean_must_be_used_on_its_own() {
    let home = tempfile::tempdir().unwrap();
    for other in ["--list", "--resume"] {
        caleb(home.path())
            .args(["--clean", other])
            .assert()
            .failure()
            .stderr(predicates::str::contains("cannot be used with"));
    }
}

#[test]
fn help_documents_clean() {
    let home = tempfile::tempdir().unwrap();
    caleb(home.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--clean"))
        .stdout(predicates::str::contains("no open tasks"));
}

#[test]
fn clean_says_session_in_the_singular() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("caleb");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("2026-05-30_09-00.md"),
        "# Session 2026-05-30 09:00\n\n## Completed\n\n- [x] done\n",
    )
    .unwrap();

    caleb(home.path())
        .arg("--clean")
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicates::str::contains("Delete 1 session? [y/N]"))
        .stdout(predicates::str::contains("Deleted 1 session."));
}
