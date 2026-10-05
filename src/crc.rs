//! The checksum `JMicron` uses in JMS578 firmware images: CRC-32 (poly 0x04C11DB7, init 0xFFFFFFFF, reflected
//! input) without output reflection or final XOR, computed over 32-bit words with their bytes reversed.

const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = table();

/// # Panics
/// If the length is not a multiple of 4.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u32 {
    assert!(bytes.len() % 4 == 0, "length must be a multiple of 4");
    let mut crc = 0xFFFF_FFFFu32;
    for word in bytes.chunks_exact(4) {
        for &byte in word.iter().rev() {
            crc = (crc << 8) ^ TABLE[((crc >> 24) ^ u32::from(byte.reverse_bits())) as usize];
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::checksum;

    /// Known answers computed with an independent bitwise implementation of the jms578flash parameters.
    #[test]
    fn known_answers() {
        assert_eq!(checksum(&[0; 4]), 0xC704_DD7B);
        assert_eq!(checksum(&(0u8..16).collect::<Vec<_>>()), 0x08FD_B614);
        assert_eq!(checksum(&[]), 0xFFFF_FFFF);
    }
}
