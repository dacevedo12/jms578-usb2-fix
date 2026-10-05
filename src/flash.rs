//! SPI NOR flash access through the bridge.

use crate::error::{Error, Result};

/// Anything that can run an SPI transaction against the bridge's flash chip.
pub trait SpiBus {
    fn max_transfer(&self) -> usize;
    fn transfer(&mut self, out: &[u8], read_count: usize) -> Result<Vec<u8>>;
}

pub const SECTOR_SIZE: usize = 0x1000;
pub const PAGE_SIZE: usize = 0x100;

/// A supported SPI NOR flash part. All use the common command set: 0x9F ID, 0x03 read, 0x05 status,
/// 0x06 write enable, 0x20 4 KB sector erase, 0x02 page program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashChip {
    pub jedec_id: [u8; 3],
    pub name: &'static str,
    pub size: usize,
    /// True when the part has been exercised on real hardware with this tool.
    pub verified_on_hardware: bool,
}

/// Parts seen on JMS578 boards: the jms578flash table, plus the Macronix part this tool was developed on.
pub const KNOWN_CHIPS: &[FlashChip] = &[
    FlashChip {
        jedec_id: [0xC2, 0x25, 0x33],
        name: "Macronix MX25U4033E",
        size: 512 << 10,
        verified_on_hardware: true,
    },
    FlashChip {
        jedec_id: [0xEF, 0x30, 0x12],
        name: "Winbond W25X20",
        size: 256 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0x5E, 0x60, 0x13],
        name: "FENTECH 25VQ40C",
        size: 512 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0x0E, 0x40, 0x12],
        name: "Fremont FT25H02",
        size: 256 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0x0E, 0x40, 0x13],
        name: "Fremont FT25H04",
        size: 512 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0xA1, 0x31, 0x11],
        name: "Fudan FM25F01",
        size: 128 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0x85, 0x40, 0x12],
        name: "PUYA P25Q21H",
        size: 256 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0x85, 0x60, 0x13],
        name: "PUYA P25D40H",
        size: 512 << 10,
        verified_on_hardware: false,
    },
    FlashChip {
        jedec_id: [0xF8, 0x42, 0x13],
        name: "Fudan FM25M04A",
        size: 512 << 10,
        verified_on_hardware: false,
    },
];

impl FlashChip {
    #[must_use]
    pub fn identify(id: [u8; 3]) -> Option<Self> {
        KNOWN_CHIPS.iter().copied().find(|c| c.jedec_id == id)
    }
}

/// Reads and writes an SPI NOR flash through an [`SpiBus`].
pub struct SpiFlash<B: SpiBus> {
    pub bus: B,
    pub chip: FlashChip,
}

impl<B: SpiBus> SpiFlash<B> {
    /// Reads the JEDEC ID and refuses unknown parts.
    pub fn new(mut bus: B) -> Result<Self> {
        let id: [u8; 3] = bus
            .transfer(&[0x9F], 3)?
            .try_into()
            .map_err(|_| Error::Transport("short ID".into()))?;
        let chip = FlashChip::identify(id).ok_or(Error::UnsupportedFlash(id))?;
        Ok(Self { bus, chip })
    }

    pub fn read(&mut self, address: usize, len: usize, progress: &mut dyn FnMut(usize)) -> Result<Vec<u8>> {
        assert!(address + len <= self.chip.size);
        let chunk = self.bus.max_transfer() - 4;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let n = chunk.min(len - out.len());
            let cmd = command(0x03, address + out.len());
            out.extend(self.bus.transfer(&cmd, n)?);
            progress(out.len());
        }
        Ok(out)
    }

    pub fn erase_sector(&mut self, address: usize) -> Result<()> {
        assert!(address % SECTOR_SIZE == 0 && address < self.chip.size);
        self.write_enable()?;
        self.bus.transfer(&command(0x20, address), 0)?;
        self.wait_ready()
    }

    /// Programs already-erased flash. Chunks never cross a page: page program wraps within its page.
    pub fn program(&mut self, address: usize, bytes: &[u8]) -> Result<()> {
        assert!(address + bytes.len() <= self.chip.size);
        let max_data = self.bus.max_transfer() - 4;
        for range in program_chunks(address, bytes.len(), max_data) {
            let data = &bytes[range.start - address..range.end - address];
            if data.iter().all(|&b| b == 0xFF) {
                continue;
            }
            self.write_enable()?;
            let mut cmd = command(0x02, range.start).to_vec();
            cmd.extend_from_slice(data);
            self.bus.transfer(&cmd, 0)?;
            self.wait_ready()?;
        }
        Ok(())
    }

    fn write_enable(&mut self) -> Result<()> {
        self.bus.transfer(&[0x06], 0).map(drop)
    }

    fn wait_ready(&mut self) -> Result<()> {
        for _ in 0..20_000 {
            if self.bus.transfer(&[0x05], 1)?[0] & 0x01 == 0 {
                return Ok(());
            }
        }
        Err(Error::FlashTimeout)
    }
}

fn command(opcode: u8, address: usize) -> [u8; 4] {
    [opcode, (address >> 16) as u8, (address >> 8) as u8, address as u8]
}

/// Splits `start..start+len` into program operations of at most `max_data` bytes that stay inside one page.
#[must_use]
pub fn program_chunks(start: usize, len: usize, max_data: usize) -> Vec<std::ops::Range<usize>> {
    let end = start + len;
    let mut chunks = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let page_end = (cursor / PAGE_SIZE + 1) * PAGE_SIZE;
        let next = end.min(cursor + max_data).min(page_end);
        chunks.push(cursor..next);
        cursor = next;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_range_and_never_cross_pages() {
        for (start, len) in [
            (0xD000, 0x200),
            (0xD0F8, 0x20),
            (0x00FF, 2),
            (0, 1),
            (0x1234, 0x777),
        ] {
            let chunks = program_chunks(start, len, 12);
            assert_eq!(chunks.first().unwrap().start, start);
            assert_eq!(chunks.last().unwrap().end, start + len);
            for pair in chunks.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "chunks must be contiguous");
            }
            for c in &chunks {
                assert!(c.len() <= 12);
                assert_eq!(
                    c.start / PAGE_SIZE,
                    (c.end - 1) / PAGE_SIZE,
                    "chunk {c:?} crosses a page"
                );
            }
        }
    }

    /// Regression: a 12-byte chunk at 0xD0FC used to straddle 0xD100 and wrap onto 0xD000.
    #[test]
    fn chunk_at_page_end_is_split() {
        let chunks = program_chunks(0xD0FC, 12, 12);
        assert_eq!(chunks, vec![0xD0FC..0xD100, 0xD100..0xD108]);
    }

    #[test]
    fn identifies_known_chips_only() {
        assert_eq!(
            FlashChip::identify([0xC2, 0x25, 0x33]).unwrap().name,
            "Macronix MX25U4033E"
        );
        assert!(FlashChip::identify([0x12, 0x34, 0x56]).is_none());
    }
}
