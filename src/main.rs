use clap::{Parser, Subcommand};
use jms578_usb2_fix::error::Error;
use jms578_usb2_fix::hw::Hardware;
use jms578_usb2_fix::hw::simulated::SimHardware;
use jms578_usb2_fix::ui::{Terminal, Ui};
use jms578_usb2_fix::wizard::{self, Outcome, Wizard};
use jms578_usb2_fix::{os, usb::UsbHardware};
use std::path::PathBuf;
use std::process::ExitCode;

/// Fix `JMicron` JMS578 USB-to-SATA adapters that fail over USB 3 by switching them to USB 2.0-only mode.
///
/// Every step is validated, a verified backup is saved before anything is written, and the change can be
/// undone with `restore`. Run with sudo: the tool must take the adapter over from the system's storage driver.
#[derive(Parser)]
#[command(version, about, long_about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Where backups are written (default: ~/JMS578-backups).
    #[arg(long, global = true)]
    backup_dir: Option<PathBuf>,
    /// Rehearse with a simulated adapter instead of real hardware. Nothing touches your devices.
    #[arg(long, global = true)]
    simulate: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Guided fix: back up, enable USB 2.0-only mode, verify (the default).
    Fix,
    /// Restore an adapter's configuration from a backup (undoes the fix).
    Restore {
        /// Backup .bin file. If omitted, choose from the backup folder.
        file: Option<PathBuf>,
    },
    /// Show the adapter's state without changing anything.
    Status,
    /// Save a verified backup without changing anything.
    Backup,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut ui = Terminal::default();
    println!(
        "jms578-usb2-fix {}{}",
        env!("CARGO_PKG_VERSION"),
        if cli.simulate { " (simulation)" } else { "" }
    );

    let backup_dir = cli.backup_dir.clone().unwrap_or_else(|| {
        if cli.simulate {
            std::env::temp_dir().join("jms578-usb2-fix-simulation")
        } else {
            os::invoking_user_home()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("JMS578-backups")
        }
    });
    let result = if cli.simulate {
        run(&cli, &mut SimHardware::default(), &mut ui, backup_dir)
    } else if !os::is_root() {
        ui.bad(
            "This tool needs administrator rights to take the adapter over from the system's storage driver.",
        );
        ui.say(&format!(
            "Run it again with sudo:  sudo {}",
            std::env::args().collect::<Vec<_>>().join(" ")
        ));
        return ExitCode::FAILURE;
    } else {
        match UsbHardware::new() {
            Ok(mut hw) => run(&cli, &mut hw, &mut ui, backup_dir),
            Err(e) => Err(e),
        }
    };
    match result {
        Ok(Outcome::Done | Outcome::NothingToDo) => ExitCode::SUCCESS,
        Ok(Outcome::Cancelled) => ExitCode::from(2),
        Err(e) => {
            ui.bad(&format!("Stopped: {e}"));
            if matches!(
                e,
                Error::Transport(_) | Error::BadStatus(_) | Error::SpiBusy | Error::ReadMismatch { .. }
            ) {
                ui.warn(
                    "The connection to the adapter is not reliable enough. Unplug and replug it, connect it over\n\
                     USB 2.0 (a USB 2.0 port or hub, or a USB 2.0 cable or adapter in between), and run the command again.",
                );
            }
            ui.say("Nothing unsafe happened: the tool only writes after every check passed, and verifies what it wrote.");
            ExitCode::FAILURE
        }
    }
}

fn run<H: Hardware>(
    cli: &Cli,
    hw: &mut H,
    ui: &mut Terminal,
    backup_dir: PathBuf,
) -> jms578_usb2_fix::error::Result<Outcome> {
    let now = humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string();
    let command = match &cli.command {
        None | Some(Command::Fix) => wizard::Command::Fix,
        Some(Command::Restore { file }) => wizard::Command::Restore(file.clone()),
        Some(Command::Status) => wizard::Command::Status,
        Some(Command::Backup) => wizard::Command::Backup,
    };
    Wizard::new(hw, ui, backup_dir, now).run(command)
}
