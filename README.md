# jms578-usb2-fix

[![CI](https://github.com/dacevedo12/jms578-usb2-fix/actions/workflows/ci.yml/badge.svg)](https://github.com/dacevedo12/jms578-usb2-fix/actions/workflows/ci.yml)

**Fix cheap USB-to-SATA adapters that don't work over USB 3**, by safely switching their JMicron **JMS578** chip
to USB 2.0-only mode. One configuration bit changes, a verified backup is made first, and it can be undone at any
time.

## Does this sound like your adapter?

- The external drive **is not recognized on a Mac** (Apple Silicon or Intel): it shows up as a USB device, but no
  disk appears in Finder or Disk Utility.
- It **works through a USB hub** or dock, but **not plugged in directly**.
- It **works on Windows** or on another computer, but not on this one.
- The disk **mounts and then disconnects**, resets in a loop, or reads fail with I/O errors.
- On Linux or a Raspberry Pi: `uas_eh_abort_handler`, `reset SuperSpeed USB device`, "Cannot enable. Maybe the USB
  cable is bad?", or it only works with `usb-storage.quirks=...:u`.

If the adapter uses a **JMicron JMS578** bridge (very common in no-name SATA cables and enclosures; USB ID
`152d:0578`, or a reused ID such as `7825:a2a4` "ULT-Best / Best USB Device"), this tool can probably fix it.
Not sure? `status` checks it without changing anything.

## Why it happens

These adapters usually have nothing wrong with the chip. The **USB 3 wiring is cheap**: the SuperSpeed pairs are
loose wires with no shield or ground. USB 3 runs at 5 Gb/s and is very sensitive to that, while USB 2 uses separate
wires that cope fine.

![Inside a cheap JMS578 adapter: USB 3 pairs as loose wires, ground pad left empty](docs/adapter-pcb.svg)

A hub happens to hide the problem, because it re-drives the signal with its own clock:

![Direct USB 3 fails, a hub re-times the signal, USB 2.0-only mode avoids the bad wires](docs/signal-path.svg)

The JMS578 firmware has a built-in **"USB 2.0 only"** option (bit 5 of configuration byte `0xF3`). With it on, the
chip never tries USB 3: it reports itself as a USB 2.0 device and uses only the wires that work. The evidence is in
[research/FINDINGS.md](research/FINDINGS.md).

**Trade-off:** the adapter then runs at USB 2.0 speed (about 35-40 MB/s) everywhere, including through hubs. If you
need USB 3 speed, the real fix is a better adapter or rewiring the cable. You can switch back at any time.

## Quick start

1. Install Rust (`curl https://sh.rustup.rs -sSf | sh`), then:

   ```sh
   cargo install --git https://github.com/dacevedo12/jms578-usb2-fix
   ```

   Or download a binary from [Releases](https://github.com/dacevedo12/jms578-usb2-fix/releases) and check it
   against its `.sha256` file. The macOS binaries are not notarized by Apple, so if you downloaded one with a
   browser, clear the quarantine flag before running it: `xattr -d com.apple.quarantine jms578-usb2-fix`.

2. Optional: rehearse the whole procedure with a simulated adapter. Nothing touches your hardware:

   ```sh
   jms578-usb2-fix --simulate
   ```

3. Connect the adapter **through a USB hub** (any USB 3 hub, dock or multiport adapter) or over USB 2.0. The
   direct USB 3 link is the broken part, so the tool needs another way in. Behind a USB 3 hub, the tool asks the
   hub to run that one port at USB 2.0 while it works (a standard USB hub request) and switches it back at the end.

4. Run it and follow the steps:

   ```sh
   sudo jms578-usb2-fix
   ```

`sudo` is needed because the tool has to take the adapter over from the system's storage driver while it works.
The drive comes back automatically afterwards.

## What it does, step by step

1. **Asks you to connect the adapter**, with its drive, through a hub or over USB 2.0. If it finds the adapter on
   a USB 3 link behind a hub, it switches that hub port to USB 2.0 (and back again when done). Plugged in directly
   over USB 3, it stops and explains what to do, since that link only ends in a disconnect loop.
2. **Offers to eject** the drive if it's mounted. Nothing proceeds until you agree.
3. **Checks, read-only, that this is something it understands.** It refuses to continue unless every check passes:
   - the bridge answers JMicron's vendor commands;
   - the flash chip is a known part;
   - the firmware header and **all of its CRCs** are valid;
   - the configuration block has the signatures the firmware requires;
   - the firmware code really contains the USB 2.0-only logic (it looks for the instructions, not just a version
     number).
4. **Saves a backup** of the firmware and configuration to `~/JMS578-backups`. The flash is read twice and the
   passes compared; the saved file is then reloaded and verified.
5. **Shows the exact change** (one byte: `0xC0 -> 0xE0`) and waits for you to type `yes`.
6. **Writes only the configuration sector**, never the firmware, then reads it back and compares.
7. **Asks you to plug the adapter in directly** and confirms it now reports USB 2.0 and the drive appears.

## Undo

```sh
sudo jms578-usb2-fix restore            # pick a backup from ~/JMS578-backups
sudo jms578-usb2-fix restore FILE.bin   # or name one
```

The restore checks that the backup came from the same adapter and firmware before writing anything.

## Other commands

| Command | What it does |
|---|---|
| `sudo jms578-usb2-fix status` | Identify the adapter and show whether USB 2.0-only mode is on. Read-only. |
| `sudo jms578-usb2-fix backup` | Save a verified backup. Read-only. |
| `--backup-dir DIR` | Use another backup folder. |
| `--simulate` | Run any command against a simulated adapter. |

## FAQ

**Can this brick my adapter?** The firmware is never touched; only the 512-byte configuration block is rewritten,
and only after every check above has passed. If a write is ever interrupted, the adapter still boots, and `restore`
puts the original back. As a last resort, JMS578 boards can be recovered by temporarily grounding the flash chip's
data-out pin (see [jms578flash](https://github.com/BertoldVdb/jms578flash#readme)).

**Is my data on the drive at risk?** The tool never reads or writes the drive; it only talks to the adapter chip.
Eject the drive first (the tool asks) so nothing is mid-copy.

**Which systems are supported?** macOS (tested on Apple Silicon). Linux builds and passes the same tests but has not
been tried on real hardware yet. Windows is not supported (the storage driver can't be detached the same way).

**My adapter uses a different chip (ASMedia, VIA...).** This tool refuses to touch it. The idea may still apply, but
those chips store their configuration differently.

**I'd rather fix the hardware.** Rewire the USB 3 pairs with a properly shielded cable and connect the drain wire
to the middle `CN2` pad, then `restore` your backup to turn USB 3 back on.

## How it was built

The investigation (USB traces, register snapshots and an 8051 disassembly of the firmware) is written up in
[research/FINDINGS.md](research/FINDINGS.md), with a small disassembler so you can check the claims on your own dump.
The tool is tested against a software model of the JMS578 and its flash chip, including the hardware quirks that
make flash writes risky; CI runs those tests on macOS and Linux, plus formatting, lint and an end-to-end
`--simulate` run.

## Credits

- [jms578flash](https://github.com/BertoldVdb/jms578flash) by Bertold Van den Bergh (MIT) documented the JMS578
  vendor commands, flash layout and firmware checksums this tool relies on.
- Found and built while rescuing a "ULT-Best" SATA cable that only worked through a hub.

## License

[MIT](LICENSE)
