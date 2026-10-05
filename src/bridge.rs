//! The `JMicron` JMS578 bridge, driven through its vendor SCSI commands.
//!
//! Command encodings follow <https://github.com/BertoldVdb/jms578flash> (MIT): opcode `0xDF` reads and writes
//! the 8051's XDATA space, and the SPI flash controller is driven in PIO mode through XDATA registers.

use crate::error::{Error, Result};
use crate::flash::SpiBus;

/// Sends SCSI commands to the bridge. Implemented over USB in `usb` and by the emulator in `sim`.
pub trait ScsiTransport {
    fn command_in(&mut self, cdb: &[u8], len: usize) -> Result<Vec<u8>>;
    fn command_out(&mut self, cdb: &[u8], data: &[u8]) -> Result<()>;
}

/// XDATA registers of the SPI controller.
pub mod reg {
    pub const SPI_OUT: u16 = 0x7140;
    pub const SPI_READ_INDEX: u16 = 0x7141;
    pub const SPI_START: u16 = 0x714C;
    pub const SPI_IN: u16 = 0x7150;
}

/// The SPI controller moves at most 16 bytes per transaction (sent + received).
pub const SPI_MAX_TRANSFER: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inquiry {
    pub vendor: String,
    pub product: String,
    pub revision: String,
}

impl Inquiry {
    #[must_use]
    pub fn drive(&self) -> String {
        format!("{} {}", self.vendor, self.product).trim().to_string()
    }
}

pub struct Bridge<T: ScsiTransport> {
    pub transport: T,
    max_busy_polls: usize,
}

impl<T: ScsiTransport> Bridge<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            max_busy_polls: 2000,
        }
    }

    /// Standard SCSI INQUIRY; the bridge answers with the attached drive's identity.
    pub fn inquiry(&mut self) -> Result<Inquiry> {
        let data = self.transport.command_in(&[0x12, 0, 0, 0, 36, 0], 36)?;
        if data.len() < 36 {
            return Err(Error::Transport("short INQUIRY reply".into()));
        }
        let text = |r: std::ops::Range<usize>| String::from_utf8_lossy(&data[r]).trim().to_string();
        Ok(Inquiry {
            vendor: text(8..16),
            product: text(16..32),
            revision: text(32..36),
        })
    }

    /// `JMicron` vendor command returning the running firmware's version word.
    pub fn firmware_version(&mut self) -> Result<[u8; 4]> {
        let data = self.transport.command_in(&[0xE0, 0xF4, 0xE7], 16)?;
        if data.len() != 16 {
            return Err(Error::Transport("short version reply".into()));
        }
        Ok(data[12..16].try_into().expect("4 bytes"))
    }

    fn xdata_read(&mut self, address: u16, count: usize) -> Result<Vec<u8>> {
        assert!((1..=255).contains(&count));
        let data = self
            .transport
            .command_in(&xdata_cdb(address, count, false), count)?;
        if data.len() != count {
            return Err(Error::Transport("short XDATA read".into()));
        }
        Ok(data)
    }

    fn xdata_write(&mut self, address: u16, bytes: &[u8]) -> Result<()> {
        assert!((1..=255).contains(&bytes.len()));
        self.transport
            .command_out(&xdata_cdb(address, bytes.len(), true), bytes)
    }
}

/// CDB for an XDATA access: `DF 00 00 00 len 00 addrH addrL 00 00 00 FD|FE`.
#[must_use]
pub fn xdata_cdb(address: u16, count: usize, write: bool) -> [u8; 12] {
    let [hi, lo] = address.to_be_bytes();
    [
        0xDF,
        0,
        0,
        0,
        count as u8,
        0,
        hi,
        lo,
        0,
        0,
        0,
        if write { 0xFE } else { 0xFD },
    ]
}

impl<T: ScsiTransport> SpiBus for Bridge<T> {
    fn max_transfer(&self) -> usize {
        SPI_MAX_TRANSFER
    }

    /// One SPI transaction: shifts out `out`, then clocks in `read_count` bytes.
    fn transfer(&mut self, out: &[u8], read_count: usize) -> Result<Vec<u8>> {
        assert!(!out.is_empty() && out.len() + read_count <= SPI_MAX_TRANSFER);
        for &byte in out {
            self.xdata_write(reg::SPI_OUT, &[byte])?;
        }
        for index in 0..read_count {
            self.xdata_write(reg::SPI_READ_INDEX, &[index as u8])?;
        }
        self.xdata_write(reg::SPI_START, &[1])?;
        let mut polls = 0;
        while self.xdata_read(reg::SPI_START, 1)?[0] != 0 {
            polls += 1;
            if polls > self.max_busy_polls {
                return Err(Error::SpiBusy);
            }
        }
        if read_count == 0 {
            Ok(Vec::new())
        } else {
            self.xdata_read(reg::SPI_IN, read_count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdata_cdb_encoding() {
        assert_eq!(
            xdata_cdb(0x7140, 1, true),
            [0xDF, 0, 0, 0, 1, 0, 0x71, 0x40, 0, 0, 0, 0xFE]
        );
        assert_eq!(
            xdata_cdb(0x7150, 12, false),
            [0xDF, 0, 0, 0, 12, 0, 0x71, 0x50, 0, 0, 0, 0xFD]
        );
    }
}
