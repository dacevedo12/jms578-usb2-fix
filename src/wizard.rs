//! The guided flows: fix (enable USB 2.0-only), restore, status and backup.

use crate::backup::Backup;
use crate::device::{Device, Report};
use crate::error::{Error, Result};
use crate::hw::{Candidate, Hardware, LinkSpeed};
use crate::nvram::{self, Nvram};
use crate::os;
use crate::ui::Ui;
use std::path::{Path, PathBuf};

/// The flows the wizard can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Fix,
    Restore(Option<PathBuf>),
    Status,
    Backup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The requested change was made and verified.
    Done,
    /// Nothing needed to change.
    NothingToDo,
    /// The user stopped before anything was written.
    Cancelled,
}

const POLL_MS: u64 = 500;

/// How to connect the adapter before working on it.
pub const CONNECT_ADVICE: &str = "Connect the adapter, with its drive attached, through a USB hub (the tool can then switch the\n\
     hub's port to USB 2.0 for you) or a USB 2.0 port. A direct USB 3 connection is exactly what is broken on\n\
     affected adapters.";

/// Shown whenever the adapter is on a USB 3 link, the path that is broken on affected adapters.
pub const USB3_LINK_STOP: &str = "The adapter is on a USB 3 link, which on affected adapters ends in a disconnect loop, so the\n\
     tool stops here. Nothing was changed. Connect it through a USB 3 hub (the tool switches the hub's port\n\
     to USB 2.0 by itself) or over USB 2.0 (a USB 2.0 port, hub, cable or adapter), then run the command again.";
const UNPLUG_TIMEOUT_POLLS: usize = 240;
const DISK_TIMEOUT_POLLS: usize = 60;

pub struct Wizard<'a, H: Hardware, U: Ui> {
    hw: &'a mut H,
    ui: &'a mut U,
    backup_dir: PathBuf,
    /// Timestamp used in backup names and metadata.
    now: String,
    /// A hub port switched to USB 2.0 by this run, to be switched back when done.
    switched_hub_port: Option<(String, u8)>,
}

impl<'a, H: Hardware, U: Ui> Wizard<'a, H, U> {
    pub fn new(hw: &'a mut H, ui: &'a mut U, backup_dir: PathBuf, now: String) -> Self {
        Self {
            hw,
            ui,
            backup_dir,
            now,
            switched_hub_port: None,
        }
    }

    /// Runs one of the flows below, always putting a switched hub port back to USB 3 afterwards.
    pub fn run(&mut self, command: Command) -> Result<Outcome> {
        let result = match command {
            Command::Fix => self.fix(),
            Command::Restore(path) => self.restore(path),
            Command::Status => self.status(),
            Command::Backup => self.backup(),
        };
        self.restore_hub_port();
        result
    }

    /// Re-enables `SuperSpeed` on a hub port this run switched to USB 2.0.
    fn restore_hub_port(&mut self) {
        if let Some(hub_port) = self.switched_hub_port.take() {
            match self.hw.restore_usb3(&hub_port) {
                Ok(()) => self.ui.good("Turned USB 3 back on for the hub port the tool had switched."),
                Err(e) => self.ui.warn(&format!(
                    "Could not turn USB 3 back on for the hub port ({e}). Unplug and replug the hub to reset it."
                )),
            }
        }
    }
    /// Back up, switch the adapter to USB 2.0-only mode, then check it works when plugged in directly.
    fn fix(&mut self) -> Result<Outcome> {
        let total = 6;
        self.ui.step(1, total, "Connect the adapter");
        self.ui.say(CONNECT_ADVICE);
        self.ui.pause("Press Enter once it is connected.");
        let Some(device) = self.select_device()? else {
            return Ok(Outcome::Cancelled);
        };
        if !self.release_disks(&device)? {
            return Ok(Outcome::Cancelled);
        }

        self.ui.step(
            2,
            total,
            "Check that this is a supported JMS578 adapter (read-only)",
        );
        let mut transport = self.hw.open(&device)?;
        let result = self.fix_with_transport(&mut transport, total);
        self.hw.close(transport);
        let outcome = result?;
        if outcome != Outcome::Done {
            return Ok(outcome);
        }

        self.restore_hub_port();
        self.ui.step(6, total, "Plug it in directly");
        self.verify_after_replug(&device, true)
    }

    fn fix_with_transport(&mut self, transport: &mut H::Transport, total: usize) -> Result<Outcome> {
        let mut dev = Device::open(TransportRef(transport))?;
        let report = self.inspect(&mut dev)?;

        self.ui.step(3, total, "Save a backup");
        let backup_path = self.save_backup(&report)?;

        if report.nvram.usb2_only() {
            self.ui
                .good("USB 2.0-only mode is already enabled on this adapter. Nothing to change.");
            return Ok(Outcome::NothingToDo);
        }

        self.ui.step(4, total, "Review the change");
        let new = report.nvram.with_usb2_only(true)?;
        let at = nvram::USB2_ONLY_OFFSET;
        self.ui.say(&format!(
            "One byte of the adapter's configuration will change:\n  \
             NVRAM 0x{at:02X}: 0x{:02X} -> 0x{:02X}  (USB 2.0-only mode: off -> on)\n\
             The firmware itself is not modified. The adapter will run at USB 2.0 speed (about 35-40 MB/s)\n\
             on every computer, including through hubs. You can undo this at any time with:\n  \
             sudo jms578-usb2-fix restore \"{}\"",
            report.nvram.bytes()[at],
            new.bytes()[at],
            backup_path.display()
        ));
        if !self.ui.confirm_word("Ready to write it to the adapter?", "yes") {
            self.ui.say("Cancelled. Nothing was written.");
            return Ok(Outcome::Cancelled);
        }

        self.ui.step(5, total, "Write and verify");
        self.ui
            .say("Writing... do not unplug the adapter until this step finishes.");
        dev.write_nvram(&new, &report.nvram)?;
        self.ui.good("Written and verified (read back and compared).");
        Ok(Outcome::Done)
    }

    /// Writes a backup's configuration back to the adapter it was taken from.
    fn restore(&mut self, path: Option<PathBuf>) -> Result<Outcome> {
        let total = 5;
        self.ui.step(1, total, "Choose the backup");
        let Some(path) = path.or_else(|| self.pick_backup()) else {
            self.ui.bad(&format!(
                "No backup selected (looked in {}).",
                self.backup_dir.display()
            ));
            return Ok(Outcome::Cancelled);
        };
        let backup = Backup::load(&path)?;
        let saved = backup.nvram()?;
        self.ui.good(&format!(
            "Backup is intact: {} (made {}, USB 2.0-only {}).",
            path.display(),
            backup.metadata.created,
            on_off(backup.metadata.usb2_only_enabled)
        ));

        self.ui.step(2, total, "Connect the adapter");
        self.ui.say(CONNECT_ADVICE);
        self.ui.pause("Press Enter once it is connected.");
        let Some(device) = self.select_device()? else {
            return Ok(Outcome::Cancelled);
        };
        if !self.release_disks(&device)? {
            return Ok(Outcome::Cancelled);
        }

        self.ui.step(
            3,
            total,
            "Check that the backup belongs to this adapter (read-only)",
        );
        let mut transport = self.hw.open(&device)?;
        let result = self.restore_with_transport(&mut transport, &backup, &saved);
        self.hw.close(transport);
        if result? != Outcome::Done {
            return Ok(Outcome::NothingToDo);
        }

        self.restore_hub_port();
        self.ui.step(5, total, "Reconnect the adapter");
        self.verify_after_replug(&device, backup.metadata.usb2_only_enabled)
    }

    fn restore_with_transport(
        &mut self,
        transport: &mut H::Transport,
        backup: &Backup,
        saved: &Nvram,
    ) -> Result<Outcome> {
        let mut dev = Device::open(TransportRef(transport))?;
        let report = self.inspect(&mut dev)?;
        backup.check_matches(&report)?;
        self.ui
            .good("Same flash chip and identical firmware as in the backup.");
        let changes = saved.differences(&report.nvram);
        if changes.is_empty() {
            self.ui
                .good("The adapter already matches the backup. Nothing to change.");
            return Ok(Outcome::NothingToDo);
        }
        self.ui.step(4, 5, "Restore");
        let list: Vec<String> = changes
            .iter()
            .map(|&i| {
                format!(
                    "NVRAM 0x{i:02X}: 0x{:02X} -> 0x{:02X}",
                    report.nvram.bytes()[i],
                    saved.bytes()[i]
                )
            })
            .collect();
        self.ui.say(&format!(
            "{} byte(s) will be restored:\n  {}\nUSB 2.0-only mode: {} -> {}",
            changes.len(),
            list.join("\n  "),
            on_off(report.nvram.usb2_only()),
            on_off(saved.usb2_only())
        ));
        if !self.ui.confirm_word("Restore this configuration?", "restore") {
            self.ui.say("Cancelled. Nothing was written.");
            return Ok(Outcome::Cancelled);
        }
        dev.write_nvram(saved, &report.nvram)?;
        self.ui.good("Restored and verified (read back and compared).");
        Ok(Outcome::Done)
    }

    /// Read-only report of the adapter's state.
    fn status(&mut self) -> Result<Outcome> {
        self.ui.step(1, 2, "Connect the adapter");
        let Some(device) = self.select_device()? else {
            return Ok(Outcome::Cancelled);
        };
        if !self.release_disks(&device)? {
            return Ok(Outcome::Cancelled);
        }
        self.ui.step(2, 2, "Inspect (read-only)");
        let mut transport = self.hw.open(&device)?;
        let result = Device::open(TransportRef(&mut transport)).and_then(|mut dev| self.inspect(&mut dev));
        self.hw.close(transport);
        let report = result?;
        self.ui.say(&format!(
            "USB 2.0-only mode is {}.",
            on_off(report.nvram.usb2_only())
        ));
        Ok(Outcome::NothingToDo)
    }

    /// Reads, validates and saves a backup without changing anything.
    fn backup(&mut self) -> Result<Outcome> {
        self.ui.step(1, 2, "Connect the adapter");
        let Some(device) = self.select_device()? else {
            return Ok(Outcome::Cancelled);
        };
        if !self.release_disks(&device)? {
            return Ok(Outcome::Cancelled);
        }
        self.ui.step(2, 2, "Read and save (read-only)");
        let mut transport = self.hw.open(&device)?;
        let result = Device::open(TransportRef(&mut transport)).and_then(|mut dev| self.inspect(&mut dev));
        self.hw.close(transport);
        self.save_backup(&result?)?;
        Ok(Outcome::Done)
    }

    fn select_device(&mut self) -> Result<Option<Candidate>> {
        for _ in 0..3 {
            let found = self.hw.candidates()?;
            match found.len() {
                0 => {
                    self.ui.warn("No USB storage device found.");
                    self.ui.pause(
                        "Check the cable and that the drive is attached, then press Enter to look again.",
                    );
                }
                1 => return Ok(self.announce(found.into_iter().next().expect("one"))),
                _ => {
                    let options: Vec<String> = found.iter().map(Candidate::describe).collect();
                    let choice = self.ui.choose(
                        "Several USB storage devices are connected. Which one is the adapter?",
                        &options,
                    );
                    return Ok(choice.and_then(|i| self.announce(found[i].clone())));
                }
            }
        }
        self.ui.bad("Still no USB storage device found.");
        Ok(None)
    }

    fn announce(&mut self, device: Candidate) -> Option<Candidate> {
        self.ui.good(&format!("Found {}.", device.describe()));
        if !matches!(device.speed, LinkSpeed::Super | LinkSpeed::SuperPlus) {
            return Some(device);
        }
        if device.usb3_hub_port.is_some() {
            self.ui.say(
                "It is on a USB 3 link through a hub, which is unreliable on affected adapters. Switching this hub\n\
                 port to USB 2.0 while the tool works; it goes back to USB 3 at the end, and nothing on the\n\
                 adapter changes.",
            );
            match self.switch_to_usb2(&device) {
                Ok(Some(moved)) => return Some(moved),
                Ok(None) => self.ui.warn("The adapter did not come back over USB 2.0."),
                Err(e) => self.ui.warn(&format!("The hub did not accept the request: {e}")),
            }
        }
        self.ui.bad(USB3_LINK_STOP);
        None
    }

    /// Switches the hub port and waits for the adapter to reappear at USB 2.0 speed.
    fn switch_to_usb2(&mut self, device: &Candidate) -> Result<Option<Candidate>> {
        self.hw.switch_to_usb2(device)?;
        for _ in 0..20 {
            self.ui.wait(POLL_MS);
            let moved = self.hw.candidates()?.into_iter().find(|c| {
                c.vendor_id == device.vendor_id
                    && c.product_id == device.product_id
                    && c.speed == LinkSpeed::High
            });
            if let Some(moved) = moved {
                self.switched_hub_port.clone_from(&device.usb3_hub_port);
                self.ui.good(&format!("Switched: {}.", moved.describe()));
                return Ok(Some(moved));
            }
        }
        Ok(None)
    }

    /// Unmounts the drive's volumes (after asking), since the system driver is about to be detached.
    fn release_disks(&mut self, device: &Candidate) -> Result<bool> {
        for disk in &device.disks {
            let volumes = self.hw.mounted_volumes(disk)?;
            if volumes.is_empty() {
                continue;
            }
            self.ui.warn(&format!(
                "The drive is mounted ({}). It must be ejected so the tool can talk to the adapter.\n\
                 Make sure nothing is copying to or from it.",
                volumes.join(", ")
            ));
            if !self.ui.confirm(&format!("Eject {disk} now?")) {
                self.ui.say("Cancelled. Nothing was changed.");
                return Ok(false);
            }
            self.hw.eject(disk)?;
            self.ui.good(&format!("Ejected {disk}."));
        }
        Ok(true)
    }

    fn inspect<T: crate::bridge::ScsiTransport>(&mut self, dev: &mut Device<T>) -> Result<Report> {
        self.ui.good(&format!(
            "Bridge answers JMicron vendor commands (firmware version {}).",
            crate::hex(&dev.firmware_version)
        ));
        self.ui.good(&format!("Attached drive: {}.", dev.drive.drive()));
        let chip = dev.flash.chip;
        self.ui.good(&format!(
            "Flash chip: {} ({} KB){}.",
            chip.name,
            chip.size >> 10,
            if chip.verified_on_hardware {
                ""
            } else {
                ", known part not yet tested with this tool"
            }
        ));
        let ui = &mut *self.ui;
        let report = dev.inspect(&mut |f| ui.progress("Reading flash twice", f))?;
        self.ui.good(&format!(
            "Firmware {} is intact (header and all CRCs verified).",
            report.firmware.version()
        ));
        self.ui.good(&format!(
            "Firmware code checks the USB 2.0-only flag ({} places, variable 0x{:04X}).",
            report.support.bit_tests, report.support.variable
        ));
        let strings = report.nvram.strings();
        self.ui.good(&format!(
            "Configuration is valid: {:04x}:{:04x}{}, USB 2.0-only {}.",
            report.nvram.vendor_id(),
            report.nvram.product_id(),
            if strings.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", strings.join(" / "))
            },
            on_off(report.nvram.usb2_only())
        ));
        Ok(report)
    }

    fn save_backup(&mut self, report: &Report) -> Result<PathBuf> {
        let backup = Backup::from_report(report, self.now.clone());
        let base = format!(
            "jms578-{}-{:04x}{:04x}",
            self.now.replace([':', '-'], ""),
            report.nvram.vendor_id(),
            report.nvram.product_id()
        );
        let path = backup.save(&self.backup_dir, &base)?;
        for p in [self.backup_dir.clone(), path.clone(), path.with_extension("json")] {
            os::chown_to_invoking_user(&p);
        }
        // Prove the saved copy is readable and intact before relying on it.
        let reloaded = Backup::load(&path)?;
        if reloaded.flash != report.flash {
            return Err(Error::InvalidBackup(
                "the saved backup does not match what was read".into(),
            ));
        }
        self.ui
            .good(&format!("Backup saved and verified: {}", path.display()));
        Ok(path)
    }

    fn pick_backup(&mut self) -> Option<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&self.backup_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bin") && p.with_extension("json").exists())
            .collect();
        found.sort();
        found.reverse();
        match found.len() {
            0 => None,
            1 => found.pop(),
            _ => {
                let names: Vec<String> = found.iter().map(|p| file_name(p)).collect();
                self.ui
                    .choose("Which backup should be restored? (newest first)", &names)
                    .map(|i| found[i].clone())
            }
        }
    }

    /// Asks for an unplug/replug and checks the adapter comes back the expected way.
    fn verify_after_replug(&mut self, before: &Candidate, expect_usb2: bool) -> Result<Outcome> {
        let same = |c: &Candidate| c.vendor_id == before.vendor_id && c.product_id == before.product_id;
        // Releasing the adapter can re-enumerate it, so take the "before" picture once that has settled.
        self.ui.wait(1500);
        let baseline: Vec<(String, u8)> = self
            .hw
            .candidates()?
            .into_iter()
            .filter(same)
            .map(|c| (c.port, c.address))
            .collect();
        self.ui.say(if expect_usb2 {
            "Unplug the adapter, then plug it DIRECTLY into the computer (no hub)."
        } else {
            "Unplug the adapter, then plug it back in."
        });
        self.ui.pause("Press Enter once it is plugged in again.");
        self.hw.replug_requested();
        // A reconnection shows up as a new port or USB address, or as the device vanishing and coming back.
        let mut seen_gone = false;
        let mut device = None;
        for _ in 0..UNPLUG_TIMEOUT_POLLS {
            let found: Vec<Candidate> = self.hw.candidates()?.into_iter().filter(same).collect();
            if found.is_empty() {
                seen_gone = true;
            } else if let Some(fresh) = found
                .into_iter()
                .find(|c| seen_gone || !baseline.contains(&(c.port.clone(), c.address)))
            {
                device = Some(fresh);
                break;
            }
            self.ui.wait(POLL_MS);
        }
        let Some(mut device) = device else {
            self.ui.warn(
                "Did not see the adapter reconnect within two minutes. The change is saved on the adapter;\n\
                 plug it in and check that the drive appears.",
            );
            return Ok(Outcome::Done);
        };
        if expect_usb2 {
            if device.usb_version == 0x0200 {
                self.ui
                    .good("The adapter now reports itself as a USB 2.0 device.");
            } else {
                self.ui.bad(&format!(
                    "The adapter still reports USB {:x}.{:02x}.",
                    device.usb_version >> 8,
                    device.usb_version & 0xFF
                ));
                return Err(Error::Io("USB 2.0-only mode did not take effect".into()));
            }
        } else {
            self.ui.good(&format!(
                "Reconnected at {}; USB 2.0-only mode is off.",
                device.speed.label()
            ));
            if matches!(device.speed, LinkSpeed::Super | LinkSpeed::SuperPlus) {
                self.ui.warn(
                    "If the drive disconnects or never mounts, this adapter's USB 3 link is unreliable: run the fix\n\
                     again with the adapter behind a USB hub.",
                );
            }
            return Ok(Outcome::Done);
        }
        for _ in 0..DISK_TIMEOUT_POLLS {
            if !device.disks.is_empty() {
                self.ui.good(&format!(
                    "Your drive is back ({}). All done.",
                    device.disks.join(", ")
                ));
                return Ok(Outcome::Done);
            }
            self.ui.wait(POLL_MS);
            if let Some(again) = self.hw.candidates()?.into_iter().find(same) {
                device = again;
            }
        }
        self.ui.warn(
            "The adapter is connected but no drive appeared yet. Check the drive's power and Disk Utility.",
        );
        Ok(Outcome::Done)
    }
}

/// Lets `Device` borrow a transport that the wizard keeps ownership of (so it can always be closed).
struct TransportRef<'a, T>(&'a mut T);

impl<T: crate::bridge::ScsiTransport> crate::bridge::ScsiTransport for TransportRef<'_, T> {
    fn command_in(&mut self, cdb: &[u8], len: usize) -> Result<Vec<u8>> {
        self.0.command_in(cdb, len)
    }
    fn command_out(&mut self, cdb: &[u8], data: &[u8]) -> Result<()> {
        self.0.command_out(cdb, data)
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
