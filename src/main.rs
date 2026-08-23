//! Binary entry point: CLI parsing and top-level dispatch.
//!
//! Everything with logic in it lives in the library crate so that tests and
//! doctests can reach it; this file only decides which entry point to call.

use anyhow::{Context, Result, bail};
use caleb::{app, clean, picker, session, storage, tui, ui};
use clap::Parser;
use std::io::Write;
use std::path::Path;

/// Help layout: name, version, and description lead, and the repository URL
/// trails the options.
///
/// The pieces come from `CARGO_PKG_*` rather than string literals so the help
/// text cannot drift from `Cargo.toml` the way the hardcoded `about` did.
const HELP_TEMPLATE: &str = "\
{name} {version}
{about}

{usage-heading} {usage}

{all-args}{after-help}
";

#[derive(Parser, Debug)]
#[command(
    name = "caleb",
    version,
    about = env!("CARGO_PKG_DESCRIPTION"),
    help_template = HELP_TEMPLATE,
    after_help = concat!("Repository: ", env!("CARGO_PKG_REPOSITORY")),
    // caleb wants -v for version; clap defaults to -V, so wire it by hand.
    disable_version_flag = true
)]
struct Cli {
    /// Pick a past session to resume (defaults to sessions with unfinished
    /// tasks; press 'a' in the picker to show all)
    #[arg(short = 'r', long = "resume")]
    resume: bool,

    /// Print all saved sessions to stdout and exit
    #[arg(long = "list")]
    list: bool,

    /// Delete saved sessions with no open tasks, after confirmation
    /// (must be used on its own)
    #[arg(long = "clean", conflicts_with_all = ["resume", "list"])]
    clean: bool,

    /// Show version and exit
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let dir = storage::default_storage_dir()
        .context("set $XDG_DATA_HOME or $HOME so caleb knows where to keep session files")?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create storage dir '{}'", dir.display()))?;

    if cli.list {
        return print_session_list(&dir);
    }

    if cli.clean {
        return clean_sessions(&dir);
    }

    if !tui::is_tty() {
        bail!("stdin or stdout is not a terminal — caleb requires an interactive TTY");
    }

    let palette = ui::Palette::new(tui::color_enabled());
    let mut tui = tui::Tui::new().context("cannot initialize the terminal")?;

    let session = if cli.resume {
        match picker::run(&dir, &mut tui, palette).context("session picker failed")? {
            picker::Choice::Cancelled => return Ok(()),
            picker::Choice::Selected(name) => {
                session::resume(&dir, &name, storage::timestamp_now())?
            }
        }
    } else {
        session::create_new(&dir, storage::timestamp_now())
            .context("cannot pick a filename for the new session")?
    };

    let mut app = app::App::new(session, dir, palette);
    app.run(&mut tui).context("session ended abnormally")
}

/// Delete every session with no open tasks, once the user says so.
///
/// Plain stdout/stdin rather than the TUI: `--clean` is a scripting-adjacent
/// path like `--list`, and a pipe that answers nothing must abort rather than
/// hang or assume yes.
fn clean_sessions(dir: &Path) -> Result<()> {
    let entries = picker::scan(dir)
        .with_context(|| format!("cannot read sessions in '{}'", dir.display()))?;
    let doomed = clean::cleanable(&entries);
    if doomed.is_empty() {
        println!("No sessions to clean — every saved session still has open tasks.");
        return Ok(());
    }

    println!("These sessions have no open tasks:");
    for e in &doomed {
        println!("  {}   {} open / {} total", e.name, e.open, e.total);
    }
    if !confirm(&format!("Delete {}? [y/N] ", plural(doomed.len())))? {
        println!("Cancelled — nothing was deleted.");
        return Ok(());
    }

    let names: Vec<&str> = doomed.iter().map(|e| e.name.as_str()).collect();
    let failures = clean::delete(dir, &names);
    println!("Deleted {}.", plural(names.len() - failures.len()));
    if failures.is_empty() {
        return Ok(());
    }
    for (name, err) in &failures {
        eprintln!("cannot delete '{name}': {err}");
    }
    bail!("{} could not be deleted", plural(failures.len()));
}

/// `1 session` / `2 sessions` — the counts here are small and user-facing.
fn plural(n: usize) -> String {
    if n == 1 {
        "1 session".to_string()
    } else {
        format!("{n} sessions")
    }
}

/// Ask once on stdin. Only a bare `y`/`yes` is a yes; EOF — a pipe with
/// nothing to say — is a no.
fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    std::io::stdout()
        .flush()
        .context("cannot write the prompt")?;

    let mut answer = String::new();
    if std::io::stdin()
        .read_line(&mut answer)
        .context("cannot read the answer")?
        == 0
    {
        return Ok(false);
    }
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

fn print_session_list(dir: &Path) -> Result<()> {
    for e in picker::scan(dir)? {
        println!("{}   {} open / {} total", e.name, e.open, e.total);
    }
    Ok(())
}
