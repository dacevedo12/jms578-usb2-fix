//! JMS578 flash layout and firmware image validation.

use crate::crc::checksum;
use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::ops::Range;

/// Where things live in the JMS578's SPI flash (as written by `JMicron`'s tools and jms578flash).
pub mod layout {
    use std::ops::Range;
    pub const METADATA: Range<usize> = 0x0000..0x0200;
    pub const HEADER: Range<usize> = 0x0E00..0x1000;
    pub const CODE: Range<usize> = 0x1000..0xD000;
    pub const NVRAM: Range<usize> = 0xD000..0xD200;
    /// The 4 KB erase sector holding the NVRAM. Everything after the NVRAM in it must be blank.
    pub const NVRAM_SECTOR: Range<usize> = 0xD000..0xE000;
    /// What the tool reads and backs up: firmware, NVRAM and the rest of its sector.
    pub const BACKUP: Range<usize> = 0x0000..0xE000;
}

/// First 0x18 header bytes of every JMS578 flash image (jms578flash `makeHeader`).
pub const EXPECTED_HEADER: &[u8; 24] = b"\x01\x00\x15\x2d\x05\x79\x03\x03\x05\x05JMicron JMS579";
pub const METADATA_MAGIC: [u8; 4] = [0x5A, 0xC3, 0x69, 0xE1];
pub const IMAGE_LEN: usize = 0xC600;
/// The boot ROM loads the program at 8051 code address 0x4000.
pub const CODE_LOAD_ADDRESS: usize = 0x4000;
const CODE_IN_IMAGE: Range<usize> = 0x400..0xC3F8;

/// A JMS578 firmware image (header | metadata | code | NVRAM), checked against its embedded CRCs.
#[derive(Debug, Clone)]
pub struct FirmwareImage {
    pub image: Vec<u8>,
}

impl FirmwareImage {
    /// Assembles the image from a flash dump starting at address 0 and validates it.
    pub fn from_flash(flash: &[u8]) -> Result<Self> {
        if flash.len() < layout::NVRAM.end {
            return Err(Error::InvalidFirmware("dump too short".into()));
        }
        let mut image = Vec::with_capacity(IMAGE_LEN);
        for range in [layout::HEADER, layout::METADATA, layout::CODE, layout::NVRAM] {
            image.extend_from_slice(&flash[range]);
        }
        let firmware = Self { image };
        firmware.validate()?;
        Ok(firmware)
    }

    fn validate(&self) -> Result<()> {
        let img = &self.image;
        let stored = |o: usize| u32::from_be_bytes(img[o..o + 4].try_into().expect("4 bytes"));
        let bad = |what: &str| Err(Error::InvalidFirmware(what.into()));
        if &img[..24] != EXPECTED_HEADER {
            return bad("unrecognized header");
        }
        if img[0x200..0x204] != METADATA_MAGIC {
            return bad("missing metadata magic");
        }
        if checksum(&img[..0x1FC]) != stored(0x1FC) {
            return bad("header checksum mismatch");
        }
        if checksum(&img[CODE_IN_IMAGE]) != stored(0xC3F8) {
            return bad("code checksum mismatch");
        }
        if full_checksum(img) != stored(0xC3FC) || full_checksum(img) != stored(0x208) {
            return bad("image checksum mismatch");
        }
        if checksum(&img[..0x3FC]) != stored(0x3FC) {
            return bad("metadata checksum mismatch");
        }
        Ok(())
    }

    #[must_use]
    pub fn code(&self) -> &[u8] {
        &self.image[CODE_IN_IMAGE]
    }

    #[must_use]
    pub fn code_sha256(&self) -> String {
        sha256_hex(self.code())
    }

    /// Version text from the header, e.g. "0103".
    #[must_use]
    pub fn version(&self) -> String {
        String::from_utf8_lossy(&self.image[0x19..0x1D]).into_owned()
    }
}

/// CRC over the image with metadata block 2 zeroed, compensated as `JMicron`'s tools do.
pub(crate) fn full_checksum(img: &[u8]) -> u32 {
    let mut zeroed = img[..0xC400].to_vec();
    zeroed[0x200..0x400].fill(0);
    checksum(&zeroed[..0xC3FC]) ^ 0x7DA4_76E9
}

/// Writes all checksums into an image (mirrors jms578flash `ChecksumUpdate`). Used to build test fixtures.
pub(crate) fn write_checksums(img: &mut [u8]) {
    let store = |o: usize, v: u32, img: &mut [u8]| img[o..o + 4].copy_from_slice(&v.to_be_bytes());
    store(0x1FC, checksum(&img[..0x1FC]), img);
    store(0xC3F8, checksum(&img[CODE_IN_IMAGE]), img);
    let full = full_checksum(img);
    store(0xC3FC, full, img);
    store(0x208, full, img);
    store(0x3FC, checksum(&img[..0x3FC]), img);
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixtures;

    #[test]
    fn synthetic_image_validates() {
        let flash = fixtures::flash(&fixtures::code(3), &fixtures::nvram(false));
        let fw = FirmwareImage::from_flash(&flash).unwrap();
        assert_eq!(fw.version(), "0103");
        assert_eq!(fw.code().len(), 0xBFF8);
    }

    #[test]
    fn any_corrupted_region_is_rejected() {
        let flash = fixtures::flash(&fixtures::code(3), &fixtures::nvram(false));
        for (address, expected) in [
            (0x0E05, "unrecognized header"),
            (0x0E40, "header checksum mismatch"),
            (0x0000, "missing metadata magic"),
            (0x0100, "metadata checksum mismatch"),
            (0x4321, "code checksum mismatch"),
        ] {
            let mut bad = flash.clone();
            bad[address] ^= 0x01;
            let err = FirmwareImage::from_flash(&bad).unwrap_err();
            assert_eq!(
                err,
                Error::InvalidFirmware(expected.into()),
                "corruption at 0x{address:04X}"
            );
        }
    }

    #[test]
    fn nvram_changes_do_not_affect_firmware_checksums() {
        let a = fixtures::flash(&fixtures::code(3), &fixtures::nvram(false));
        let b = fixtures::flash(&fixtures::code(3), &fixtures::nvram(true));
        assert!(FirmwareImage::from_flash(&a).is_ok() && FirmwareImage::from_flash(&b).is_ok());
        assert_eq!(a[layout::CODE], b[layout::CODE]);
    }
}
