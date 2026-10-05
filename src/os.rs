//! Platform glue: privileges, the invoking user's home, and finding/ejecting the adapter's disks.

use crate::error::{Error, Result};
use std::path::PathBuf;
use std::process::Command;

/// True when running as root (needed to detach the system's storage driver).
#[must_use]
#[allow(unsafe_code)]
pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Home directory of the user who ran `sudo`, falling back to `$HOME`.
#[must_use]
#[allow(unsafe_code)]
pub fn invoking_user_home() -> Option<PathBuf> {
    if let Ok(user) = std::env::var("SUDO_USER") {
        let name = std::ffi::CString::new(user).ok()?;
        // SAFETY: `name` is a valid NUL-terminated string; the returned record is read before any other
        // passwd call and its `pw_dir` is copied out immediately.
        let home = unsafe {
            let pw = libc::getpwnam(name.as_ptr());
            if pw.is_null() || (*pw).pw_dir.is_null() {
                return None;
            }
            std::ffi::CStr::from_ptr((*pw).pw_dir)
                .to_string_lossy()
                .into_owned()
        };
        return Some(PathBuf::from(home));
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Gives a file created under sudo back to the invoking user.
pub fn chown_to_invoking_user(path: &std::path::Path) {
    let id = |var| std::env::var(var).ok().and_then(|v| v.parse::<u32>().ok());
    if let (Some(uid), Some(gid)) = (id("SUDO_UID"), id("SUDO_GID")) {
        let _ = std::os::unix::fs::chown(path, Some(uid), Some(gid));
    }
}

fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(Error::Io(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::run;
    use crate::error::{Error, Result};
    use plist::Value;

    /// libusb on macOS derives bus and ports from the `IOKit` locationID; rebuild it from "bus-p1.p2...".
    fn location_id(port: &str) -> Option<i64> {
        let (bus, path) = port.split_once('-')?;
        let mut id = bus.parse::<i64>().ok()? << 24;
        for (i, p) in path.split('.').enumerate() {
            id |= p.parse::<i64>().ok()? << (20 - 4 * i);
        }
        Some(id)
    }

    fn whole_disks(node: &Value, out: &mut Vec<String>) {
        let Some(dict) = node.as_dictionary() else { return };
        if dict.get("Whole").and_then(Value::as_boolean) == Some(true)
            && let Some(name) = dict.get("BSD Name").and_then(Value::as_string)
        {
            out.push(name.to_string());
        }
        if let Some(children) = dict.get("IORegistryEntryChildren").and_then(Value::as_array) {
            for child in children {
                whole_disks(child, out);
            }
        }
    }

    #[must_use]
    pub fn disks_for_usb_device(port: &str, _vid: u16, _pid: u16) -> Vec<String> {
        let Some(location) = location_id(port) else {
            return Vec::new();
        };
        let Ok(xml) = run("ioreg", &["-a", "-r", "-c", "IOUSBHostDevice", "-l"]) else {
            return Vec::new();
        };
        let Ok(Value::Array(devices)) = Value::from_reader_xml(xml.as_bytes()) else {
            return Vec::new();
        };
        let mut disks = Vec::new();
        for device in &devices {
            let id = device
                .as_dictionary()
                .and_then(|d| d.get("locationID"))
                .and_then(Value::as_signed_integer);
            if id == Some(location) {
                whole_disks(device, &mut disks);
            }
        }
        disks.dedup();
        disks
    }

    /// Mount points of the disk's partitions and of APFS volumes stored on it.
    #[must_use]
    pub fn mounted_volumes(disk: &str) -> Vec<String> {
        let Ok(xml) = run("diskutil", &["list", "-plist"]) else {
            return Vec::new();
        };
        let Ok(root) = Value::from_reader_xml(xml.as_bytes()) else {
            return Vec::new();
        };
        let Some(all) = root
            .as_dictionary()
            .and_then(|d| d.get("AllDisksAndPartitions"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        let ours = |id: &str| id == disk || id.strip_prefix(disk).is_some_and(|rest| rest.starts_with('s'));
        let mut mounts = Vec::new();
        let collect = |entries: Option<&Value>, mounts: &mut Vec<String>| {
            for e in entries.and_then(Value::as_array).into_iter().flatten() {
                if let Some(mp) = e
                    .as_dictionary()
                    .and_then(|d| d.get("MountPoint"))
                    .and_then(Value::as_string)
                {
                    mounts.push(mp.to_string());
                }
            }
        };
        for entry in all.iter().filter_map(Value::as_dictionary) {
            let id = entry
                .get("DeviceIdentifier")
                .and_then(Value::as_string)
                .unwrap_or_default();
            let on_disk = ours(id)
                || entry
                    .get("APFSPhysicalStores")
                    .and_then(Value::as_array)
                    .is_some_and(|stores| {
                        stores.iter().any(|s| {
                            s.as_dictionary()
                                .and_then(|d| d.get("DeviceIdentifier"))
                                .and_then(Value::as_string)
                                .is_some_and(ours)
                        })
                    });
            if on_disk {
                collect(entry.get("Partitions"), &mut mounts);
                collect(entry.get("APFSVolumes"), &mut mounts);
                if let Some(mp) = entry.get("MountPoint").and_then(Value::as_string) {
                    mounts.push(mp.to_string());
                }
            }
        }
        mounts
    }

    pub fn eject(disk: &str) -> Result<()> {
        run("diskutil", &["eject", disk])
            .map(drop)
            .map_err(|e| Error::Io(format!("could not eject {disk}: {e}")))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::run;
    use crate::error::{Error, Result};
    use std::path::Path;

    #[must_use]
    pub fn disks_for_usb_device(port: &str, _vid: u16, _pid: u16) -> Vec<String> {
        let Ok(usb) = std::fs::canonicalize(Path::new("/sys/bus/usb/devices").join(port)) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir("/sys/block") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| std::fs::canonicalize(e.path()).is_ok_and(|p| p.starts_with(&usb)))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[must_use]
    pub fn mounted_volumes(disk: &str) -> Vec<String> {
        let device = format!("/dev/{disk}");
        std::fs::read_to_string("/proc/mounts")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut fields = l.split_whitespace();
                let (source, target) = (fields.next()?, fields.next()?);
                source.starts_with(&device).then(|| target.replace("\\040", " "))
            })
            .collect()
    }

    pub fn eject(disk: &str) -> Result<()> {
        for mount in mounted_volumes(disk) {
            run("umount", &[&mount]).map_err(|e| Error::Io(format!("could not unmount {mount}: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use crate::error::Result;
    pub fn disks_for_usb_device(_: &str, _: u16, _: u16) -> Vec<String> {
        Vec::new()
    }
    pub fn mounted_volumes(_: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn eject(_: &str) -> Result<()> {
        Ok(())
    }
}

pub use platform::{disks_for_usb_device, eject, mounted_volumes};
