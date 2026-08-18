//! Binary entry point: CLI parsing and top-level dispatch.
//!
//! Everything with logic in it lives in the library crate so that tests and
//! doctests can reach it; this file only decides which entry point to call.

use anyhow::{Context, Result, bail};
use caleb::{app, picker, session, storage, tui, ui};
use clap::Parser;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    name = "caleb",
    version,
    about = "Track tasks for a coding session",
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

fn print_session_list(dir: &Path) -> Result<()> {
    for e in picker::scan(dir)? {
        println!("{}   {} open / {} total", e.name, e.open, e.total);
    }
    Ok(())
}
