//! Checks, by inspecting the 8051 code itself, that a firmware honours the NVRAM USB 2.0-only flag.
//!
//! The analysed firmware copies NVRAM byte 0xF3 (XDATA 0x3BF3) into a variable and tests bit 5 of it before
//! link training, while building the device descriptor (bcdUSB) and while building the BOS descriptor.

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usb2OnlySupport {
    /// XDATA address of the variable holding the flag.
    pub variable: u16,
    /// Number of places that test bit 5 of the variable.
    pub bit_tests: usize,
}

/// Minimum number of bit-5 tests required to trust the flag (the analysed firmware has three).
pub const REQUIRED_BIT_TESTS: usize = 2;

pub fn usb2_only_support(code: &[u8]) -> Result<Usb2OnlySupport> {
    // MOV DPTR,#3BF3h ; MOVX A,@DPTR ; MOV DPTR,#var ; MOVX @DPTR,A
    let loads = find(
        code,
        &[
            Some(0x90),
            Some(0x3B),
            Some(0xF3),
            Some(0xE0),
            Some(0x90),
            None,
            None,
            Some(0xF0),
        ],
    );
    let [load] = loads[..] else {
        return Err(Error::UnsupportedFirmware(format!(
            "NVRAM byte 0xF3 is not loaded the expected way ({} matches)",
            loads.len()
        )));
    };
    let (hi, lo) = (code[load + 5], code[load + 6]);
    // MOV DPTR,#var ; MOVX A,@DPTR ; JNB|JB ACC.5,rel
    let tests = [0x30, 0x20]
        .iter()
        .map(|&op| {
            find(
                code,
                &[Some(0x90), Some(hi), Some(lo), Some(0xE0), Some(op), Some(0xE5)],
            )
            .len()
        })
        .sum();
    let support = Usb2OnlySupport {
        variable: u16::from_be_bytes([hi, lo]),
        bit_tests: tests,
    };
    if tests < REQUIRED_BIT_TESTS {
        return Err(Error::UnsupportedFirmware(format!(
            "flag variable 0x{:04X} is tested {tests} time(s)",
            support.variable
        )));
    }
    Ok(support)
}

fn find(data: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
    data.windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| {
            pattern
                .iter()
                .zip(w.iter())
                .all(|(p, b)| p.is_none_or(|p| p == *b))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixtures;

    #[test]
    fn finds_flag_variable_and_tests() {
        let support = usb2_only_support(&fixtures::code(3)).unwrap();
        assert_eq!(
            support,
            Usb2OnlySupport {
                variable: 0x4420,
                bit_tests: 3
            }
        );
    }

    #[test]
    fn rejects_firmware_without_evidence() {
        assert!(
            usb2_only_support(&fixtures::code(1)).is_err(),
            "one test is not enough"
        );
        assert!(usb2_only_support(&fixtures::code_without_flag_load()).is_err());
    }
}
