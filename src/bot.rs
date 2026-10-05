//! USB Mass Storage Bulk-Only Transport framing (USB MSC BOT 1.0, section 5).

use crate::error::{Error, Result};

pub const CBW_SIGNATURE: u32 = 0x4342_5355; // "USBC"
pub const CSW_SIGNATURE: u32 = 0x5342_5355; // "USBS"
pub const CBW_LEN: usize = 31;
pub const CSW_LEN: usize = 13;

/// Builds a Command Block Wrapper.
#[must_use]
pub fn command_wrapper(tag: u32, cdb: &[u8], data_len: u32, data_in: bool) -> [u8; CBW_LEN] {
    assert!((1..=16).contains(&cdb.len()), "CDB must be 1..=16 bytes");
    let mut cbw = [0u8; CBW_LEN];
    cbw[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
    cbw[4..8].copy_from_slice(&tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&data_len.to_le_bytes());
    cbw[12] = if data_in { 0x80 } else { 0x00 };
    cbw[14] = cdb.len() as u8;
    cbw[15..15 + cdb.len()].copy_from_slice(cdb);
    cbw
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub tag: u32,
    pub residue: u32,
    pub status: u8,
}

/// Parses and validates a Command Status Wrapper.
pub fn parse_status(bytes: &[u8], expected_tag: u32) -> Result<Status> {
    if bytes.len() != CSW_LEN {
        return Err(Error::BadStatus(format!("length {}", bytes.len())));
    }
    let word = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
    if word(0) != CSW_SIGNATURE {
        return Err(Error::BadStatus("bad signature".into()));
    }
    let status = Status {
        tag: word(4),
        residue: word(8),
        status: bytes[12],
    };
    if status.tag != expected_tag {
        return Err(Error::BadStatus(format!("tag {} != {expected_tag}", status.tag)));
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wrapper_layout() {
        let cbw = command_wrapper(0x0102_0304, &[0x12, 0, 0, 0, 36, 0], 36, true);
        assert_eq!(&cbw[0..4], b"USBC");
        assert_eq!(&cbw[4..8], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&cbw[8..12], &[36, 0, 0, 0]);
        assert_eq!(cbw[12], 0x80);
        assert_eq!(cbw[14], 6);
        assert_eq!(&cbw[15..21], &[0x12, 0, 0, 0, 36, 0]);
        assert!(cbw[21..].iter().all(|&b| b == 0));
    }

    #[test]
    fn status_parsing() {
        let mut csw = [0u8; CSW_LEN];
        csw[0..4].copy_from_slice(b"USBS");
        csw[4..8].copy_from_slice(&7u32.to_le_bytes());
        csw[12] = 1;
        assert_eq!(parse_status(&csw, 7).unwrap().status, 1);
        assert!(parse_status(&csw, 8).is_err(), "tag mismatch must fail");
        csw[0] = b'X';
        assert!(parse_status(&csw, 7).is_err(), "bad signature must fail");
        assert!(parse_status(&csw[..12], 7).is_err(), "short reply must fail");
    }
}
