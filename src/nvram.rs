//! The JMS578's 512-byte configuration block (flash 0xD000, loaded to XDATA 0x3B00 at boot).
//!
//! Only fields the firmware was shown to read are interpreted; see `research/FINDINGS.md`.

use crate::error::{Error, Result};

pub const SIZE: usize = 0x200;
/// Byte whose bit 5 makes the firmware skip `SuperSpeed`, report bcdUSB 2.00 and omit the USB 3 BOS descriptor.
pub const USB2_ONLY_OFFSET: usize = 0xF3;
pub const USB2_ONLY_MASK: u8 = 0x20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nvram {
    bytes: Vec<u8>,
}

impl Nvram {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != SIZE {
            return Err(Error::InvalidNvram(format!("size {}", bytes.len())));
        }
        if bytes.iter().all(|&b| b == 0xFF) {
            return Err(Error::InvalidNvram("the block is blank".into()));
        }
        // The firmware ignores the whole block unless bytes 0xFE-0xFF read "JM".
        if &bytes[0xFE..0x100] != b"JM" {
            return Err(Error::InvalidNvram("missing \"JM\" signature".into()));
        }
        Ok(Self {
            bytes: bytes.to_vec(),
        })
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn vendor_id(&self) -> u16 {
        u16::from_le_bytes([self.bytes[0], self.bytes[1]])
    }

    #[must_use]
    pub fn product_id(&self) -> u16 {
        u16::from_le_bytes([self.bytes[2], self.bytes[3]])
    }

    /// The device-option block at 0xE0 is only applied by the firmware when it begins with "HD".
    #[must_use]
    pub fn has_device_options(&self) -> bool {
        &self.bytes[0xE0..0xE2] == b"HD"
    }

    #[must_use]
    pub fn usb2_only(&self) -> bool {
        self.has_device_options() && self.bytes[USB2_ONLY_OFFSET] & USB2_ONLY_MASK != 0
    }

    /// Returns a copy with the USB 2.0-only flag set or cleared. Nothing else changes.
    pub fn with_usb2_only(&self, enabled: bool) -> Result<Self> {
        if !self.has_device_options() {
            return Err(Error::InvalidNvram(
                "no \"HD\" option block, so the firmware would ignore the flag".into(),
            ));
        }
        let mut bytes = self.bytes.clone();
        if enabled {
            bytes[USB2_ONLY_OFFSET] |= USB2_ONLY_MASK;
        } else {
            bytes[USB2_ONLY_OFFSET] &= !USB2_ONLY_MASK;
        }
        Self::parse(&bytes)
    }

    /// Offsets whose values differ from `other`.
    #[must_use]
    pub fn differences(&self, other: &Self) -> Vec<usize> {
        (0..SIZE).filter(|&i| self.bytes[i] != other.bytes[i]).collect()
    }

    /// USB string descriptors (manufacturer, product, serial), decoded best-effort for display.
    #[must_use]
    pub fn strings(&self) -> Vec<String> {
        let mut found = Vec::new();
        let mut offset = 0x0C;
        while offset + 2 < 0xC0 {
            let len = self.bytes[offset] as usize;
            if len > 2 && self.bytes[offset + 1] == 0x03 && offset + len <= 0xC0 {
                let text: Vec<u8> = self.bytes[offset + 2..offset + len]
                    .iter()
                    .copied()
                    .take_while(|&b| b != 0)
                    .collect();
                if !text.is_empty() && text.iter().all(|b| (0x20..0x7F).contains(b)) {
                    found.push(String::from_utf8_lossy(&text).into_owned());
                }
                offset += len;
            } else {
                offset += 1;
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixtures;

    #[test]
    fn parses_fields() {
        let nv = Nvram::parse(&fixtures::nvram(false)).unwrap();
        assert_eq!((nv.vendor_id(), nv.product_id()), (0x7825, 0xA2A4));
        assert!(nv.has_device_options());
        assert!(!nv.usb2_only());
        assert_eq!(nv.strings(), vec!["ACME0000", "Bridge01"]);
    }

    #[test]
    fn toggling_changes_exactly_one_bit() {
        let off = Nvram::parse(&fixtures::nvram(false)).unwrap();
        let on = off.with_usb2_only(true).unwrap();
        assert!(on.usb2_only());
        assert_eq!(on.differences(&off), vec![USB2_ONLY_OFFSET]);
        assert_eq!(
            on.bytes()[USB2_ONLY_OFFSET] ^ off.bytes()[USB2_ONLY_OFFSET],
            USB2_ONLY_MASK
        );
        assert_eq!(on.with_usb2_only(false).unwrap(), off);
    }

    #[test]
    fn rejects_blocks_the_firmware_would_ignore() {
        assert!(Nvram::parse(&[0xFF; SIZE]).is_err());
        assert!(Nvram::parse(&[0x00; 10]).is_err());
        let mut no_signature = fixtures::nvram(false);
        no_signature[0xFE] = 0;
        assert!(Nvram::parse(&no_signature).is_err());
        let mut no_options = fixtures::nvram(false);
        no_options[0xE0] = 0;
        let nv = Nvram::parse(&no_options).unwrap();
        assert!(nv.with_usb2_only(true).is_err());
    }
}
