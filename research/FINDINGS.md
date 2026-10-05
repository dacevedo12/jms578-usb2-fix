# Findings: the JMS578 "USB 2.0 only" option

This documents how the fix was found and how to verify it on your own adapter. Addresses refer to
firmware version `0103` with code SHA-256 `3121f87261424975ae17240994dc8c78ead91ec68717c54cc312326e9b381fdc`
(the hash `jms578-usb2-fix` prints in backups as `firmware_code_sha256`). Other firmware builds may place
things elsewhere, which is why the tool checks the code for the pattern instead of trusting these addresses.

## The adapter

- USB ID `7825:a2a4`, strings "ULT-Best" / "Best USB Device". `7825:a2a4` is OWC's ID for its PA023U3 cable and is
  widely reused by clones.
- Board marked `HZ-01`: JMicron **JMS578** bridge, Macronix **MX25U4033E** SPI flash (512 KB, JEDEC `C2 25 33`),
  30 MHz crystal.
- The USB 3 SuperSpeed pairs are loose wires soldered to a 5-pad header (`CN2`). The middle pad, the ground drain
  for the pairs, has no wire attached. See [the diagram](../docs/adapter-pcb.svg).

## Symptoms and what they ruled out

Plugged directly into a Mac (Apple Silicon, both USB-C ports, both plug orientations), the adapter enumerates at
5 Gb/s, answers INQUIRY, TEST UNIT READY and READ CAPACITY, then every data read times out and the device resets in a
loop. The port's `link-error-count` climbs into the tens of thousands. Through a USB 3 hub it works.

| Experiment | Result |
|---|---|
| Disable USB 3 link power management (`kUSBHostDeviceDisablePortLPM`) | no change |
| Force Bulk-Only instead of UAS (own BOT driver in user space) | 512-byte reads work, 1024-byte reads fail |
| Force single-packet bursts (`bMaxBurst = 0`) | same |
| Flip the USB-C plug (other SuperSpeed lane pair) | same |

A full-size 1024-byte SuperSpeed packet almost never arrives intact while a short one often does. That points at
the physical USB 3 link (signal integrity), not at a protocol, power or driver issue. A hub works because it
recovers the clock and re-drives a clean signal. USB 2.0 uses separate wires (D+/D−) that tolerate this board.

## Flash layout

As in [jms578flash](https://github.com/BertoldVdb/jms578flash):

| Flash range | Contents |
|---|---|
| `0x0000-0x01FF` | metadata block (magic `5A C3 69 E1`, CRCs) |
| `0x0E00-0x0FFF` | header (`01 00 15 2D 05 79 03 03 05 05` "JMicron JMS579", version) |
| `0x1000-0xCFFF` | 8051 program, loaded at code address `0x4000` |
| `0xD000-0xD1FF` | NVRAM (configuration), loaded to XDATA `0x3B00` at boot |
| `0xF000-0xF0FF` | written by the firmware itself (serial number, attached drive model) |

All firmware CRCs follow jms578flash's `ChecksumUpdate`: CRC-32 (poly `0x04C11DB7`, init `0xFFFFFFFF`, reflected
input, **no** output reflection or final XOR) over 32-bit words with their bytes reversed. The NVRAM is **not**
covered by any checksum; the firmware only checks its signatures.

## NVRAM fields the firmware reads

| Offset | Meaning (as used by the code) |
|---|---|
| `0x00-0x03` | USB vendor and product ID, little endian |
| `0x0C...` | USB string descriptors (manufacturer, product, serial) |
| `0xE0-0xE1` | `"HD"`: the option block below is only applied when present |
| `0xE7` | copied to `0x441F` |
| `0xF0-0xF1` | `"BC"`: second option block signature |
| `0xF2` | feature bits (bits 4, 5, 6, 7 set various flags) |
| **`0xF3`** | copied to **`0x4420`**: **bit 5 = USB 2.0 only**; bits 0, 2, 3, 6 and 7 are tested elsewhere (not analysed) |
| `0xF4-0xF7` | 32-bit timer value |
| `0xF8`, `0xFA`, `0xFB` | PHY setup selector and two more options |
| `0xFE-0xFF` | `"JM"`: without it the whole NVRAM is ignored |

The NVRAM parser copies the byte at `0x5D04`:

```
5D04  MOV DPTR,#3BF3h     ; NVRAM byte 0xF3
5D07  MOVX A,@DPTR
5D08  MOV DPTR,#4420h
5D0B  MOVX @DPTR,A
```

## What bit 5 of `0x4420` does

It is tested in exactly three places:

1. **USB link state machine** (state byte in internal RAM `0x50`, nine states dispatched by a jump table at
   `0x4D54`). State 5 decides whether to try SuperSpeed:

   ```
   4D7F  MOV DPTR,#44B9h ; MOVX A,@DPTR ; JNZ 4D97h     ; forced USB 2 flag
   4D85  MOV DPTR,#4420h ; MOVX A,@DPTR ; JNB ACC.5,4D8Eh
   4D8C  SJMP 4D97h                                      ; bit 5 set -> USB 2 path
   4D8E  MOV DPTR,#44C5h ; ... SUBB A,#0Ah ; JC 4D9Dh   ; fewer than 10 USB 3 failures -> try USB 3
   4D97  MOV 50h,#06h                                    ; state 6: USB 2 path
   4D9D  MOV 50h,#07h                                    ; state 7: power up the USB 3 PHY
   ```

   The USB 2 path ends up in a routine at `0x5194` that writes `7072 <- 01` and sets bit 5 of register `0x7280`,
   which keeps the USB 3 PHY off. Register snapshots confirm it: `0x7280` reads `0x29` while connected at USB 2 and
   `0x09` at USB 3.
2. **BOS descriptor** at `0xCF67`: with bit 5 set, the USB 3 capability descriptor is not built.
3. **Device descriptor** at `0xD60E`: with bit 5 set, bcdUSB at XDATA `0x4052` is set to `0x0200`.

So setting NVRAM `0xF3` bit 5 makes the chip present itself as a plain USB 2.0 device and never train a
SuperSpeed link. The host then uses the USB 2 wires only.

## Reproduce it

```sh
sudo jms578-usb2-fix backup                       # saves ~/JMS578-backups/<name>.bin
python3 research/d8051.py ~/JMS578-backups/<name>.bin --xref 3BF3
python3 research/d8051.py ~/JMS578-backups/<name>.bin --xref 4420
python3 research/d8051.py ~/JMS578-backups/<name>.bin --list --linear --from 4D6F --to 4DA2
JMS578_DUMP=~/JMS578-backups/<name>.bin cargo test --test real_dump -- --ignored --nocapture
```

## Result

With `0xF3` changed from `0xC0` to `0xE0`, the adapter enumerates directly on the Mac at 480 Mb/s with bcdUSB 2.00,
Apple's Bulk-Only driver attaches, and the APFS volume mounts with no read errors.

## Notes for further research

- Reading large XDATA ranges blindly (for example 255 bytes from `0x7000`) can hang the bridge until it is power
  cycled. The tool only touches the four SPI controller registers documented by jms578flash.
- Short vendor commands work even over the broken direct USB 3 link, but not reliably enough to write flash. Use
  a hub or a USB 2.0 port when changing the configuration.
