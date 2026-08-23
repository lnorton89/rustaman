// ============================================================================
// Module:       rustaman (binary crate root)
// Description:  Entry point: parses the command line, loads preferences, and
//               hands off to the desktop front end.
//
// Dependencies: clap (argument parsing), anyhow; rustaman::gui
// ============================================================================

// No console window behind the GUI in a release build. Windows gives a
// console-subsystem binary a terminal whether or not it writes to one, so
// launching from Explorer would open an empty black window beside the app
// and leave it there for the session.
//
// Debug builds keep the console deliberately: it is where panics and
// `println!` go while developing, and a GUI-subsystem binary discards
// both silently.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Entry point for `rustaman`.
//!
//! Deliberately thin. It parses the command line, loads the saved
//! preferences, and hands off — the window, the sampler, and everything
//! else belong to the library, so this file is the only part that would
//! differ if the app were ever driven another way.
//!
//! ## Failures are reported in a dialog, not on stderr
//!
//! A release build is a GUI-subsystem binary and has no console: anything
//! written to stderr goes nowhere, so a startup failure would look like
//! the program simply refusing to open. See [`report`].

/// The command line.
///
/// Deliberately small. A task manager is not a command-line tool, and
/// every flag added here is a flag that has to keep working. The two that
/// exist are the two that cannot be reached any other way: a theme
/// override for someone testing one, and a reset for a config file that
/// has somehow made the app unusable.
#[derive(clap::Parser, Debug)]
#[command(name = "rustaman", version, about = crate::TAGLINE)]
struct Cli {
    /// Select this theme for the session and save it as the new preference.
    #[arg(long, value_name = "ID")]
    theme: Option<String>,

    /// Start with the built-in defaults and overwrite the saved
    /// preferences on exit.
    ///
    /// The escape hatch for a config that has made the window unusable —
    /// a saved size larger than any attached monitor, say. Without this
    /// the only fix is finding and deleting a file whose location the
    /// user has no reason to know.
    #[arg(long)]
    reset: bool,
}

/// The one-line description, taken from the brand so the `--help` text
/// and the about panel cannot disagree.
const TAGLINE: &str = rustaman::brand::TAGLINE;

/// The message a non-Windows build prints instead of opening a window.
///
/// The crate's portable half genuinely builds and tests anywhere — see
/// the crate docs — so this explains what did and did not happen rather
/// than failing to link with a page of missing symbols.
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "Rustaman is a Windows task manager: it reads the machine through \
         interfaces no other platform has.\n\
         The portable half of the crate still builds and tests here — try \
         `cargo test --lib`."
    );
    std::process::ExitCode::FAILURE
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    {
        use clap::Parser;

        let cli = Cli::parse();
        let mut config = if cli.reset {
            rustaman::config::Config::default()
        } else {
            rustaman::config::load()
        };
        if let Some(theme) = cli.theme {
            config.theme = Some(theme);
        }

        match rustaman::gui::run(config) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                report(&error.to_string());
                std::process::ExitCode::FAILURE
            }
        }
    }
}

/// Reports a startup failure somewhere the user will actually see it.
///
/// A release build has no console (see the `windows_subsystem` attribute
/// above), so stderr goes nowhere and a failure would look like the app
/// refusing to start for no reason. A message box is the only channel
/// that works whether the app was launched from Explorer or a terminal —
/// and for a desktop app it is the right one either way.
#[cfg(windows)]
fn report(message: &str) {
    // Still written to stderr as well, for the debug build and for anyone
    // running it from a terminal that does have one.
    eprintln!("{message}");
    rustaman::win::dialog::show_error(message, rustaman::brand::NAME);
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Result};
    use clap::Parser as _;

    #[test]
    fn the_command_line_parses_with_no_arguments() -> Result<()> {
        // The overwhelmingly common case: double-clicked from Explorer.
        let cli = Cli::try_parse_from(["rustaman"])
            .map_err(|error| anyhow!("the bare invocation must parse: {error}"))?;
        assert_eq!(cli.theme, None);
        assert!(!cli.reset);
        Ok(())
    }

    #[test]
    fn a_theme_override_is_taken() -> Result<()> {
        let cli = Cli::try_parse_from(["rustaman", "--theme", "nebula"])
            .map_err(|error| anyhow!("--theme should parse: {error}"))?;
        assert_eq!(cli.theme.as_deref(), Some("nebula"));
        Ok(())
    }

    #[test]
    fn reset_is_a_flag_rather_than_a_value() -> Result<()> {
        let cli = Cli::try_parse_from(["rustaman", "--reset"])
            .map_err(|error| anyhow!("--reset should parse: {error}"))?;
        assert!(cli.reset);
        Ok(())
    }

    #[test]
    fn an_unknown_flag_is_rejected_rather_than_ignored() {
        // A typo'd flag that is silently ignored is a flag that appears
        // not to work.
        assert!(Cli::try_parse_from(["rustaman", "--nope"]).is_err());
    }

    #[test]
    fn the_help_text_and_the_about_panel_agree() {
        assert_eq!(TAGLINE, rustaman::brand::TAGLINE);
    }
}
