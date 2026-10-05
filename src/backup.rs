//! Verified backups: `<name>.bin` (flash 0x0000..0xE000) plus `<name>.json` metadata.

use crate::device::Report;
use crate::error::{Error, Result};
use crate::firmware::{FirmwareImage, layout, sha256_hex};
use crate::nvram::Nvram;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub format: u32,
    pub tool: String,
    pub created: String,
    pub flash_chip: String,
    pub flash_jedec: String,
    pub flash_offset: usize,
    pub flash_length: usize,
    pub flash_sha256: String,
    pub firmware_version: String,
    pub firmware_code_sha256: String,
    pub vendor_id: String,
    pub product_id: String,
    pub usb2_only_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct Backup {
    pub flash: Vec<u8>,
    pub metadata: Metadata,
}

impl Backup {
    #[must_use]
    pub fn from_report(report: &Report, created: String) -> Self {
        Self {
            flash: report.flash.clone(),
            metadata: Metadata {
                format: 1,
                tool: concat!("jms578-usb2-fix ", env!("CARGO_PKG_VERSION")).into(),
                created,
                flash_chip: report.chip.name.into(),
                flash_jedec: crate::hex(&report.chip.jedec_id),
                flash_offset: layout::BACKUP.start,
                flash_length: report.flash.len(),
                flash_sha256: sha256_hex(&report.flash),
                firmware_version: report.firmware.version(),
                firmware_code_sha256: report.firmware.code_sha256(),
                vendor_id: format!("{:04x}", report.nvram.vendor_id()),
                product_id: format!("{:04x}", report.nvram.product_id()),
                usb2_only_enabled: report.nvram.usb2_only(),
            },
        }
    }

    pub fn nvram(&self) -> Result<Nvram> {
        Nvram::parse(&self.flash[layout::NVRAM])
    }

    /// Writes `<base>.bin` and `<base>.json` into `dir` (never overwriting). Returns the .bin path.
    pub fn save(&self, dir: &Path, base: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let bin = dir.join(format!("{base}.bin"));
        let json = bin.with_extension("json");
        if bin.exists() || json.exists() {
            return Err(Error::Io(format!("{} already exists", bin.display())));
        }
        std::fs::write(&bin, &self.flash)?;
        let text = serde_json::to_string_pretty(&self.metadata).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::write(&json, text + "\n")?;
        Ok(bin)
    }

    /// Loads a backup and checks it end to end: hash, size, firmware CRCs and NVRAM format.
    pub fn load(bin: &Path) -> Result<Self> {
        let invalid = |d: String| Error::InvalidBackup(d);
        let flash = std::fs::read(bin).map_err(|e| invalid(format!("cannot read {}: {e}", bin.display())))?;
        let json_path = bin.with_extension("json");
        let json = std::fs::read_to_string(&json_path)
            .map_err(|e| invalid(format!("cannot read {}: {e}", json_path.display())))?;
        let metadata: Metadata =
            serde_json::from_str(&json).map_err(|e| invalid(format!("bad metadata: {e}")))?;
        if metadata.format != 1
            || metadata.flash_offset != layout::BACKUP.start
            || flash.len() != metadata.flash_length
            || flash.len() != layout::BACKUP.len()
        {
            return Err(invalid("unexpected size or format".into()));
        }
        if sha256_hex(&flash) != metadata.flash_sha256 {
            return Err(invalid("checksum mismatch, the file is corrupted".into()));
        }
        let firmware = FirmwareImage::from_flash(&flash)?;
        if firmware.code_sha256() != metadata.firmware_code_sha256 {
            return Err(invalid("firmware hash does not match its metadata".into()));
        }
        let backup = Self { flash, metadata };
        backup.nvram()?;
        Ok(backup)
    }

    /// Confirms this backup belongs to the adapter described by `report`: same flash chip, same firmware code.
    pub fn check_matches(&self, report: &Report) -> Result<()> {
        if self.metadata.flash_jedec != crate::hex(&report.chip.jedec_id) {
            return Err(Error::BackupMismatch(format!(
                "flash chip {} vs {}",
                self.metadata.flash_jedec,
                crate::hex(&report.chip.jedec_id)
            )));
        }
        if self.metadata.firmware_code_sha256 != report.firmware.code_sha256() {
            return Err(Error::BackupMismatch("the firmware code differs".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::sim::{Simulator, fixtures};

    fn report(code_tests: usize) -> Report {
        let sim = Simulator::new(fixtures::flash(
            &fixtures::code(code_tests),
            &fixtures::nvram(false),
        ));
        Device::open(sim).unwrap().inspect(&mut |_| {}).unwrap()
    }

    #[test]
    fn round_trip_and_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let backup = Backup::from_report(&report(3), "2026-10-05T00:00:00Z".into());
        let path = backup.save(dir.path(), "test").unwrap();
        assert!(backup.save(dir.path(), "test").is_err(), "must never overwrite");
        let loaded = Backup::load(&path).unwrap();
        assert_eq!(loaded.metadata, backup.metadata);
        assert_eq!(loaded.flash, backup.flash);

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0xD010] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(Backup::load(&path), Err(Error::InvalidBackup(_))));
    }

    #[test]
    fn rejects_backup_from_other_firmware() {
        let backup = Backup::from_report(&report(3), "x".into());
        assert!(backup.check_matches(&report(3)).is_ok());
        assert!(matches!(
            backup.check_matches(&report(4)),
            Err(Error::BackupMismatch(_))
        ));
    }
}
