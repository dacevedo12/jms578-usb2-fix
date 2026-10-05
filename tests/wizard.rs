//! End-to-end runs of the guided flows against the simulated adapter, as a user would experience them.

use jms578_usb2_fix::error::Error;
use jms578_usb2_fix::firmware::layout;
use jms578_usb2_fix::hw::simulated::SimHardware;
use jms578_usb2_fix::nvram::USB2_ONLY_OFFSET;
use jms578_usb2_fix::sim::{Simulator, fixtures};
use jms578_usb2_fix::ui::Scripted;
use jms578_usb2_fix::wizard::{Command, Outcome, Wizard};
use std::path::{Path, PathBuf};

const NOW: &str = "2026-10-05T12:00:00Z";

fn adapter(code: &[u8], usb2_only: bool) -> SimHardware {
    SimHardware::new(Simulator::new(fixtures::flash(code, &fixtures::nvram(usb2_only))))
}

fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "bin"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn changed_bytes(before: &[u8], after: &[u8]) -> Vec<usize> {
    (0..before.len()).filter(|&i| before[i] != after[i]).collect()
}

#[test]
fn fix_backs_up_changes_one_byte_and_verifies_after_replug() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "yes"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();

    assert_eq!(outcome, Outcome::Done, "{}", ui.output());
    assert_eq!(
        changed_bytes(&original, &hw.sim.flash_contents()),
        vec![layout::NVRAM.start + USB2_ONLY_OFFSET]
    );
    let saved = backups(dir.path());
    assert_eq!(saved.len(), 1);
    assert_eq!(
        std::fs::read(&saved[0]).unwrap(),
        original[layout::BACKUP].to_vec(),
        "backup holds the original"
    );
    let out = ui.output();
    for expected in [
        "Firmware 0103 is intact",
        "Backup saved and verified",
        "now reports itself as a USB 2.0 device",
        "Your drive is back",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

#[test]
fn declining_to_eject_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["n"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(hw.sim.flash_contents(), original);
    assert_eq!(backups(dir.path()), Vec::<PathBuf>::new());
}

#[test]
fn not_typing_yes_writes_nothing_but_keeps_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "y"]); // "y" is not "yes"
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(hw.sim.flash_contents(), original);
    assert_eq!(backups(dir.path()).len(), 1);
}

#[test]
fn already_enabled_adapter_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), true);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::NothingToDo);
    assert_eq!(hw.sim.flash_contents(), original);
}

#[test]
fn firmware_without_the_flag_is_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code_without_flag_load(), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "yes"]);
    let err = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap_err();
    assert!(matches!(err, Error::UnsupportedFirmware(_)), "{err}");
    assert_eq!(hw.sim.flash_contents(), original);
}

#[test]
fn restore_undoes_the_fix_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "yes"]);
    Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    let backup = backups(dir.path()).remove(0);

    let mut ui = Scripted::new(&["y", "restore"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Restore(Some(backup)))
        .unwrap();
    assert_eq!(outcome, Outcome::Done, "{}", ui.output());
    assert_eq!(
        hw.sim.flash_contents(),
        original,
        "restore must bring back the original bytes"
    );
    assert!(ui.output().contains("USB 2.0-only mode: on -> off"));
}

#[test]
fn restore_picks_the_backup_from_the_folder_when_not_given() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "yes"]);
    Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    let mut ui = Scripted::new(&["y", "restore"]);
    Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Restore(None))
        .unwrap();
    assert_eq!(hw.sim.flash_contents(), original);
}

#[test]
fn restore_refuses_a_backup_from_different_firmware() {
    let dir = tempfile::tempdir().unwrap();
    let mut other = adapter(&fixtures::code(4), false);
    let mut ui = Scripted::new(&["y"]);
    Wizard::new(&mut other, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Backup)
        .unwrap();
    let foreign = backups(dir.path()).remove(0);

    let mut hw = adapter(&fixtures::code(3), true);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "restore"]);
    let err = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Restore(Some(foreign)))
        .unwrap_err();
    assert!(matches!(err, Error::BackupMismatch(_)), "{err}");
    assert_eq!(hw.sim.flash_contents(), original);
}

#[test]
fn status_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Status)
        .unwrap();
    assert_eq!(outcome, Outcome::NothingToDo);
    assert_eq!(hw.sim.flash_contents(), original);
    assert!(ui.output().contains("USB 2.0-only mode is off"));
}

#[test]
fn direct_usb3_link_stops_before_touching_anything() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    hw.through_hub = false; // plugged in directly...
    hw.usb3_link = true; // ...and linked at 5 Gb/s
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&[]); // no question is asked
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(ui.output().contains("The adapter is on a USB 3 link"));
    assert_eq!(hw.sim.flash_contents(), original);
    assert_eq!(backups(dir.path()), Vec::<PathBuf>::new());
}

#[test]
fn usb3_link_through_a_hub_is_switched_to_usb2_automatically() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    hw.usb3_link = true; // trains at 5 Gb/s through the hub, like the real adapter in USB 3 mode
    let original = hw.sim.flash_contents();
    let mut ui = Scripted::new(&["y", "yes"]); // eject, confirm write: the hub switch needs no answer
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::Done, "{}", ui.output());
    assert!(
        ui.output()
            .contains("Switched: Bridge01 (simulated) (7825:a2a4), 480 Mb/s (USB 2), through a hub.")
    );
    assert_eq!(
        changed_bytes(&original, &hw.sim.flash_contents()),
        vec![layout::NVRAM.start + USB2_ONLY_OFFSET]
    );
}

#[test]
fn replugging_before_pressing_enter_is_still_detected() {
    let dir = tempfile::tempdir().unwrap();
    let mut hw = adapter(&fixtures::code(3), false);
    hw.fast_replug = true;
    let mut ui = Scripted::new(&["y", "yes"]);
    let outcome = Wizard::new(&mut hw, &mut ui, dir.path().into(), NOW.into())
        .run(Command::Fix)
        .unwrap();
    assert_eq!(outcome, Outcome::Done, "{}", ui.output());
    let out = ui.output();
    assert!(out.contains("now reports itself as a USB 2.0 device"), "{out}");
    assert!(!out.contains("Did not see the adapter reconnect"), "{out}");
}
