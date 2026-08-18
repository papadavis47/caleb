//! Behavior of the binary itself — argument handling and the non-TTY paths.
//!
//! `cargo test` runs with stdout captured, so every invocation here is
//! non-interactive by construction. That is exactly the surface worth pinning:
//! the TUI paths need a pty (see `scripts/smoke.py`), but these do not, and
//! they are the ones a user hits by piping or scripting caleb.

use assert_cmd::Command;

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
