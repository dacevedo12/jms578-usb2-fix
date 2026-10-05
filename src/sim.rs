//! A software model of a JMS578 adapter: vendor SCSI commands, the PIO SPI controller registers and an SPI NOR
//! flash with realistic semantics (write-enable latch, busy status, erase to 0xFF, programming only clears bits,
//! page program wraps within its 256-byte page). Used by the test suite and by `--simulate`.

use crate::bridge::{ScsiTransport, reg};
use crate::error::{Error, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct Flash {
    memory: Vec<u8>,
    jedec: [u8; 3],
    write_enabled: bool,
    busy_polls: u32,
    programs_to_drop: u32,
}

impl Flash {
    fn execute(&mut self, out: &[u8], read_count: usize) -> Vec<u8> {
        let address = || (usize::from(out[1]) << 16) | (usize::from(out[2]) << 8) | usize::from(out[3]);
        match out[0] {
            0x9F => {
                return self
                    .jedec
                    .iter()
                    .copied()
                    .chain(std::iter::repeat(0))
                    .take(read_count)
                    .collect();
            }
            0x03 => {
                let start = address();
                return (0..read_count)
                    .map(|i| self.memory[(start + i) % self.memory.len()])
                    .collect();
            }
            0x05 => {
                let status = u8::from(self.busy_polls > 0) | if self.write_enabled { 0x02 } else { 0 };
                self.busy_polls = self.busy_polls.saturating_sub(1);
                return std::iter::once(status)
                    .chain(std::iter::repeat(0))
                    .take(read_count)
                    .collect();
            }
            0x06 => self.write_enabled = true,
            0x20 if self.write_enabled => {
                let base = address() / 0x1000 * 0x1000;
                self.memory[base..base + 0x1000].fill(0xFF);
                self.write_enabled = false;
                self.busy_polls = 3;
            }
            0x02 if self.write_enabled => {
                self.write_enabled = false;
                if self.programs_to_drop > 0 {
                    self.programs_to_drop -= 1;
                } else {
                    let start = address();
                    let page = start / 0x100 * 0x100;
                    for (i, &byte) in out[4..].iter().enumerate() {
                        self.memory[page + (start - page + i) % 0x100] &= byte;
                    }
                    self.busy_polls = 1;
                }
            }
            _ => {}
        }
        vec![0; read_count]
    }
}

struct State {
    flash: Flash,
    xdata: HashMap<u16, u8>,
    spi_out: Vec<u8>,
    spi_read_count: usize,
    spi_busy: u32,
    read_transactions: usize,
    corrupt_read_transaction: Option<usize>,
}

/// A simulated adapter. Clones share the same hardware state.
#[derive(Clone)]
pub struct Simulator(Rc<RefCell<State>>);

impl Simulator {
    #[must_use]
    pub fn new(flash: Vec<u8>) -> Self {
        Self::with_jedec(flash, [0xC2, 0x25, 0x33])
    }

    #[must_use]
    pub fn with_jedec(memory: Vec<u8>, jedec: [u8; 3]) -> Self {
        Self(Rc::new(RefCell::new(State {
            flash: Flash {
                memory,
                jedec,
                write_enabled: false,
                busy_polls: 0,
                programs_to_drop: 0,
            },
            xdata: HashMap::new(),
            spi_out: Vec::new(),
            spi_read_count: 0,
            spi_busy: 0,
            read_transactions: 0,
            corrupt_read_transaction: None,
        })))
    }

    #[must_use]
    pub fn flash_contents(&self) -> Vec<u8> {
        self.0.borrow().flash.memory.clone()
    }

    /// Flips one byte in the reply of the nth flash read transaction (simulates a flaky USB link).
    pub fn corrupt_read_transaction(&self, n: usize) {
        let mut s = self.0.borrow_mut();
        s.corrupt_read_transaction = Some(s.read_transactions + n);
    }

    /// Makes the next `n` page-program operations silently do nothing.
    pub fn drop_program_operations(&self, n: u32) {
        self.0.borrow_mut().flash.programs_to_drop = n;
    }

    fn xdata_read(s: &mut State, address: u16) -> u8 {
        if address == reg::SPI_START {
            s.spi_busy = s.spi_busy.saturating_sub(1);
            return u8::from(s.spi_busy > 0);
        }
        s.xdata.get(&address).copied().unwrap_or(0)
    }

    fn xdata_write(s: &mut State, address: u16, value: u8) {
        match address {
            reg::SPI_OUT => s.spi_out.push(value),
            reg::SPI_READ_INDEX => s.spi_read_count = s.spi_read_count.max(usize::from(value) + 1),
            reg::SPI_START if value == 1 => {
                let out = std::mem::take(&mut s.spi_out);
                let mut reply = s.flash.execute(&out, s.spi_read_count);
                if out[0] == 0x03 {
                    s.read_transactions += 1;
                    if Some(s.read_transactions) == s.corrupt_read_transaction && !reply.is_empty() {
                        reply[0] ^= 0xFF;
                    }
                }
                for (i, byte) in reply.into_iter().enumerate() {
                    s.xdata.insert(reg::SPI_IN + i as u16, byte);
                }
                s.spi_read_count = 0;
                s.spi_busy = 2;
            }
            _ => {
                s.xdata.insert(address, value);
            }
        }
    }
}

impl ScsiTransport for Simulator {
    fn command_in(&mut self, cdb: &[u8], len: usize) -> Result<Vec<u8>> {
        let mut s = self.0.borrow_mut();
        match cdb {
            [0x12, ..] => {
                let mut reply = vec![b' '; 36];
                reply[8..16].copy_from_slice(b"KINGSTON");
                reply[16..28].copy_from_slice(b"SA400S37480G");
                reply[32..36].copy_from_slice(b"4101");
                Ok(reply)
            }
            [0xE0, 0xF4, 0xE7, ..] => Ok([0; 12].into_iter().chain([0xAE, 0x01, 0x00, 0x01]).collect()),
            [0xDF, .., 0xFD] => {
                let address = u16::from_be_bytes([cdb[6], cdb[7]]);
                Ok((0..len)
                    .map(|i| Self::xdata_read(&mut s, address + i as u16))
                    .collect())
            }
            _ => Err(Error::CommandFailed {
                opcode: cdb[0],
                status: 1,
            }),
        }
    }

    fn command_out(&mut self, cdb: &[u8], data: &[u8]) -> Result<()> {
        let mut s = self.0.borrow_mut();
        if cdb.len() != 12 || cdb[0] != 0xDF || cdb[11] != 0xFE || usize::from(cdb[4]) != data.len() {
            return Err(Error::CommandFailed {
                opcode: cdb[0],
                status: 1,
            });
        }
        let address = u16::from_be_bytes([cdb[6], cdb[7]]);
        for (i, &byte) in data.iter().enumerate() {
            Self::xdata_write(&mut s, address + i as u16, byte);
        }
        Ok(())
    }
}

/// Synthetic, CRC-valid JMS578 flash contents. Contains no `JMicron` code.
pub mod fixtures {
    use crate::firmware::{EXPECTED_HEADER, IMAGE_LEN, METADATA_MAGIC, write_checksums};

    const CODE_LEN: usize = 0xC000 - 8;

    /// 8051 code with the flag copied to 0x4420 and bit 5 of it tested `bit_tests` times.
    #[must_use]
    pub fn code(bit_tests: usize) -> Vec<u8> {
        let mut code = vec![0u8; CODE_LEN];
        code[0x100..0x108].copy_from_slice(&[0x90, 0x3B, 0xF3, 0xE0, 0x90, 0x44, 0x20, 0xF0]);
        for i in 0..bit_tests {
            let at = 0x200 + i * 0x40;
            code[at..at + 7].copy_from_slice(&[0x90, 0x44, 0x20, 0xE0, 0x30, 0xE5, 0x05]);
        }
        code
    }

    #[must_use]
    pub fn code_without_flag_load() -> Vec<u8> {
        let mut code = code(3);
        code[0x100..0x108].fill(0);
        code
    }

    #[must_use]
    pub fn nvram(usb2_only: bool) -> Vec<u8> {
        let mut nv = vec![0u8; 0x200];
        nv[0..4].copy_from_slice(&[0x25, 0x78, 0xA4, 0xA2]);
        nv[0x0C..0x16].copy_from_slice(b"\x0A\x03ACME0000");
        nv[0x30..0x3A].copy_from_slice(b"\x0A\x03Bridge01");
        nv[0xE0..0xE2].copy_from_slice(b"HD");
        nv[0xF0..0xF2].copy_from_slice(b"BC");
        nv[0xF3] = if usb2_only { 0xE0 } else { 0xC0 };
        nv[0xFE..0x100].copy_from_slice(b"JM");
        nv
    }

    /// A 512 KB flash laid out like a real JMS578 board.
    #[must_use]
    pub fn flash(code: &[u8], nvram: &[u8]) -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        image[..24].copy_from_slice(EXPECTED_HEADER);
        image[0x18] = 1;
        image[0x19..0x1D].copy_from_slice(b"0103");
        image[0x200..0x204].copy_from_slice(&METADATA_MAGIC);
        image[0x400..0x400 + code.len()].copy_from_slice(code);
        image[0xC400..0xC600].copy_from_slice(nvram);
        write_checksums(&mut image);

        let mut flash = vec![0xFFu8; 512 << 10];
        flash[0x0E00..0x1000].copy_from_slice(&image[..0x200]);
        flash[..0x200].copy_from_slice(&image[0x200..0x400]);
        flash[0x1000..0xD000].copy_from_slice(&image[0x400..0xC400]);
        flash[0xD000..0xD200].copy_from_slice(&image[0xC400..]);
        flash
    }
}
