//! Validated, high-level operations on a JMS578 adapter.

use crate::analysis::{self, Usb2OnlySupport};
use crate::bridge::{Bridge, Inquiry, ScsiTransport};
use crate::error::{Error, Result};
use crate::firmware::{FirmwareImage, layout};
use crate::flash::{FlashChip, SpiFlash};
use crate::nvram::{self, Nvram};

/// Everything learned about an adapter from a validated read of its flash.
#[derive(Debug, Clone)]
pub struct Report {
    pub chip: FlashChip,
    pub firmware_version: [u8; 4],
    pub drive: Inquiry,
    /// Flash contents over [`layout::BACKUP`], read twice and compared.
    pub flash: Vec<u8>,
    pub firmware: FirmwareImage,
    pub nvram: Nvram,
    pub support: Usb2OnlySupport,
}

pub struct Device<T: ScsiTransport> {
    pub flash: SpiFlash<Bridge<T>>,
    pub drive: Inquiry,
    pub firmware_version: [u8; 4],
}

impl<T: ScsiTransport> Device<T> {
    /// Identifies the bridge (INQUIRY, `JMicron` version command, flash JEDEC ID). Read-only.
    pub fn open(transport: T) -> Result<Self> {
        let mut bridge = Bridge::new(transport);
        let drive = bridge.inquiry()?;
        let firmware_version = bridge.firmware_version()?;
        let flash = SpiFlash::new(bridge)?;
        Ok(Self {
            flash,
            drive,
            firmware_version,
        })
    }

    /// Reads the flash twice, compares the passes, and validates firmware, NVRAM and USB 2.0-only support.
    /// Never writes. `progress` receives a fraction in 0.0..=1.0.
    pub fn inspect(&mut self, progress: &mut dyn FnMut(f64)) -> Result<Report> {
        let range = layout::BACKUP;
        let total = (range.len() * 2) as f64;
        let first = self
            .flash
            .read(range.start, range.len(), &mut |n| progress(n as f64 / total))?;
        let second = self.flash.read(range.start, range.len(), &mut |n| {
            progress((range.len() + n) as f64 / total);
        })?;
        if let Some(offset) = first.iter().zip(&second).position(|(a, b)| a != b) {
            return Err(Error::ReadMismatch { offset });
        }
        let firmware = FirmwareImage::from_flash(&first)?;
        let nvram = Nvram::parse(&first[layout::NVRAM])?;
        if first[layout::NVRAM.end..layout::NVRAM_SECTOR.end]
            .iter()
            .any(|&b| b != 0xFF)
        {
            return Err(Error::UnexpectedFlashContents(
                "data after the NVRAM in its erase sector".into(),
            ));
        }
        let support = analysis::usb2_only_support(firmware.code())?;
        Ok(Report {
            chip: self.flash.chip,
            firmware_version: self.firmware_version,
            drive: self.drive.clone(),
            flash: first,
            firmware,
            nvram,
            support,
        })
    }

    /// Replaces the NVRAM, but only if its sector still holds exactly `expected` followed by blank space.
    /// Erases the 4 KB sector, programs it and verifies by reading back. Retries the whole cycle once.
    pub fn write_nvram(&mut self, new: &Nvram, expected: &Nvram) -> Result<()> {
        let sector = layout::NVRAM_SECTOR;
        let current = self.flash.read(sector.start, sector.len(), &mut |_| {})?;
        if current[..nvram::SIZE] != *expected.bytes() {
            return Err(Error::UnexpectedFlashContents(
                "the NVRAM changed since it was inspected".into(),
            ));
        }
        if current[nvram::SIZE..].iter().any(|&b| b != 0xFF) {
            return Err(Error::UnexpectedFlashContents(
                "data after the NVRAM in its erase sector".into(),
            ));
        }
        let mut last = Error::FlashVerifyFailed {
            address: sector.start,
        };
        for _ in 0..2 {
            match self.write_and_verify(new) {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn write_and_verify(&mut self, new: &Nvram) -> Result<()> {
        let sector = layout::NVRAM_SECTOR;
        self.flash.erase_sector(sector.start)?;
        self.flash.program(sector.start, new.bytes())?;
        let read_back = self.flash.read(sector.start, sector.len(), &mut |_| {})?;
        let expected = |i: usize| if i < nvram::SIZE { new.bytes()[i] } else { 0xFF };
        match (0..read_back.len()).find(|&i| read_back[i] != expected(i)) {
            Some(offset) => Err(Error::FlashVerifyFailed {
                address: sector.start + offset,
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Simulator, fixtures};

    fn device(sim: &Simulator) -> Device<Simulator> {
        Device::open(sim.clone()).unwrap()
    }

    #[test]
    fn inspect_reports_validated_state() {
        let sim = Simulator::new(fixtures::flash(&fixtures::code(3), &fixtures::nvram(false)));
        let report = device(&sim).inspect(&mut |_| {}).unwrap();
        assert_eq!(report.chip.name, "Macronix MX25U4033E");
        assert_eq!(report.drive.drive(), "KINGSTON SA400S37480G");
        assert!(!report.nvram.usb2_only());
        assert_eq!(report.support.bit_tests, 3);
    }

    #[test]
    fn inconsistent_reads_are_detected() {
        let sim = Simulator::new(fixtures::flash(&fixtures::code(3), &fixtures::nvram(false)));
        sim.corrupt_read_transaction(5000);
        assert!(matches!(
            device(&sim).inspect(&mut |_| {}),
            Err(Error::ReadMismatch { .. })
        ));
    }

    #[test]
    fn enabling_usb2_only_changes_exactly_one_flash_byte() {
        let original = fixtures::flash(&fixtures::code(3), &fixtures::nvram(false));
        let sim = Simulator::new(original.clone());
        let mut dev = device(&sim);
        let report = dev.inspect(&mut |_| {}).unwrap();
        dev.write_nvram(&report.nvram.with_usb2_only(true).unwrap(), &report.nvram)
            .unwrap();
        let after = sim.flash_contents();
        let changed: Vec<usize> = (0..after.len()).filter(|&i| after[i] != original[i]).collect();
        assert_eq!(changed, vec![layout::NVRAM.start + nvram::USB2_ONLY_OFFSET]);
        assert!(
            Device::open(sim.clone())
                .unwrap()
                .inspect(&mut |_| {})
                .unwrap()
                .nvram
                .usb2_only()
        );
    }

    #[test]
    fn transient_program_failure_is_retried() {
        let sim = Simulator::new(fixtures::flash(&fixtures::code(3), &fixtures::nvram(false)));
        let mut dev = device(&sim);
        let report = dev.inspect(&mut |_| {}).unwrap();
        sim.drop_program_operations(1);
        dev.write_nvram(&report.nvram.with_usb2_only(true).unwrap(), &report.nvram)
            .unwrap();
        assert!(
            sim.flash_contents()[layout::NVRAM.start + nvram::USB2_ONLY_OFFSET] & nvram::USB2_ONLY_MASK != 0
        );
    }

    #[test]
    fn refuses_when_nvram_changed_since_inspection() {
        let sim = Simulator::new(fixtures::flash(&fixtures::code(3), &fixtures::nvram(false)));
        let mut dev = device(&sim);
        let report = dev.inspect(&mut |_| {}).unwrap();
        let stale = Nvram::parse(&fixtures::nvram(true)).unwrap();
        let before = sim.flash_contents();
        let err = dev
            .write_nvram(&report.nvram.with_usb2_only(true).unwrap(), &stale)
            .unwrap_err();
        assert!(matches!(err, Error::UnexpectedFlashContents(_)));
        assert_eq!(sim.flash_contents(), before, "nothing may be written");
    }

    #[test]
    fn refuses_unknown_flash_chip() {
        let sim = Simulator::with_jedec(
            fixtures::flash(&fixtures::code(3), &fixtures::nvram(false)),
            [1, 2, 3],
        );
        assert!(matches!(
            Device::open(sim),
            Err(Error::UnsupportedFlash([1, 2, 3]))
        ));
    }

    #[test]
    fn refuses_data_after_nvram_in_sector() {
        let mut flash = fixtures::flash(&fixtures::code(3), &fixtures::nvram(false));
        flash[0xD800] = 0x00;
        let sim = Simulator::new(flash);
        assert!(matches!(
            device(&sim).inspect(&mut |_| {}),
            Err(Error::UnexpectedFlashContents(_))
        ));
    }
}
