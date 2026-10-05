//! Switch `JMicron` JMS578 USB-to-SATA bridges to USB 2.0-only mode, safely.
//!
//! The firmware supports a "USB 2.0 only" option stored in bit 5 of NVRAM byte 0xF3. Setting it makes adapters
//! whose USB 3 wiring is faulty enumerate as reliable USB 2.0 devices. See `research/FINDINGS.md`.

pub mod analysis;
pub mod backup;
pub mod bot;
pub mod bridge;
pub mod crc;
pub mod device;
pub mod error;
pub mod firmware;
pub mod flash;
pub mod hw;
pub mod nvram;
pub mod os;
pub mod sim;
pub mod ui;
pub mod usb;
pub mod wizard;

/// Space-separated uppercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
