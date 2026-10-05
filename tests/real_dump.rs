//! Validates the analysis against a real flash dump. Dumps are never committed (they contain proprietary firmware),
//! so this test is ignored by default:
//!
//! ```sh
//! JMS578_DUMP=path/to/backup.bin cargo test --test real_dump -- --ignored
//! ```

use jms578_usb2_fix::{analysis, firmware::FirmwareImage, firmware::layout, nvram::Nvram};

#[test]
#[ignore = "needs JMS578_DUMP pointing at a backup .bin"]
fn real_dump_validates() {
    let path = std::env::var("JMS578_DUMP").expect("set JMS578_DUMP");
    let flash = std::fs::read(path).unwrap();
    let firmware = FirmwareImage::from_flash(&flash).expect("firmware CRCs");
    let support = analysis::usb2_only_support(firmware.code()).expect("USB 2.0-only support");
    let nvram = Nvram::parse(&flash[layout::NVRAM]).expect("NVRAM");
    println!(
        "firmware {} code sha256 {} flag variable 0x{:04X} tested {}x, USB 2.0-only: {}",
        firmware.version(),
        firmware.code_sha256(),
        support.variable,
        support.bit_tests,
        nvram.usb2_only()
    );
}
