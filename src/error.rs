//! Error type. Messages are written for end users.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    BadStatus(String),
    CommandFailed { opcode: u8, status: u8 },
    SpiBusy,
    UnsupportedFlash([u8; 3]),
    FlashTimeout,
    FlashVerifyFailed { address: usize },
    InvalidFirmware(String),
    InvalidNvram(String),
    UnsupportedFirmware(String),
    UnexpectedFlashContents(String),
    ReadMismatch { offset: usize },
    InvalidBackup(String),
    BackupMismatch(String),
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(d) => write!(f, "USB communication failed: {d}"),
            Self::BadStatus(d) => write!(f, "the adapter sent a malformed status reply: {d}"),
            Self::CommandFailed { opcode, status } => {
                write!(f, "the adapter rejected command 0x{opcode:02X} (status {status})")
            }
            Self::SpiBusy => write!(f, "the adapter's flash controller stayed busy"),
            Self::UnsupportedFlash(id) => {
                write!(
                    f,
                    "unknown flash chip (JEDEC ID {}); nothing was written",
                    crate::hex(id)
                )
            }
            Self::FlashTimeout => write!(f, "the flash chip did not finish an operation in time"),
            Self::FlashVerifyFailed { address } => write!(f, "flash verification failed at 0x{address:05X}"),
            Self::InvalidFirmware(d) => write!(f, "this does not look like intact JMS578 firmware: {d}"),
            Self::InvalidNvram(d) => write!(
                f,
                "the adapter's configuration (NVRAM) is not in the expected format: {d}"
            ),
            Self::UnsupportedFirmware(d) => {
                write!(
                    f,
                    "this firmware has not been verified to support USB 2.0-only mode: {d}"
                )
            }
            Self::UnexpectedFlashContents(d) => {
                write!(f, "unexpected flash contents, refusing to write: {d}")
            }
            Self::ReadMismatch { offset } => write!(
                f,
                "two reads of the flash disagree at 0x{offset:05X}; the USB connection is unreliable"
            ),
            Self::InvalidBackup(d) => write!(f, "the backup is not valid: {d}"),
            Self::BackupMismatch(d) => write!(
                f,
                "this backup was made from a different adapter or firmware: {d}"
            ),
            Self::Io(d) => write!(f, "{d}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<rusb::Error> for Error {
    fn from(e: rusb::Error) -> Self {
        Self::Transport(e.to_string())
    }
}
