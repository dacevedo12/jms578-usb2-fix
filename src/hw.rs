//! The hardware the wizard talks to: real USB devices, or a simulated adapter.

use crate::bridge::ScsiTransport;
use crate::error::Result;

/// How a device is currently connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSpeed {
    Low,
    Full,
    High,
    Super,
    SuperPlus,
    Unknown,
}

impl LinkSpeed {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "1.5 Mb/s (USB 1)",
            Self::Full => "12 Mb/s (USB 1)",
            Self::High => "480 Mb/s (USB 2)",
            Self::Super => "5 Gb/s (USB 3)",
            Self::SuperPlus => "10 Gb/s (USB 3)",
            Self::Unknown => "unknown speed",
        }
    }
}

/// A USB mass-storage device that might be a JMS578 adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Stable identity of the physical port, e.g. "2-1.3".
    pub port: String,
    /// USB address; changes whenever the device re-enumerates (for example after a replug).
    pub address: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
    /// bcdUSB from the device descriptor: 0x0200 once USB 2.0-only mode is active.
    pub usb_version: u16,
    pub speed: LinkSpeed,
    pub behind_hub: bool,
    /// When linked at USB 3 through a USB 3 hub: that hub (by port path) and the port the adapter is on.
    /// The hub can be told to disable `SuperSpeed` on that port, which moves the adapter to USB 2.0.
    pub usb3_hub_port: Option<(String, u8)>,
    /// Whole-disk device names belonging to it ("disk4", "sdb").
    pub disks: Vec<String>,
}

impl Candidate {
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} ({:04x}:{:04x}), {}{}",
            if self.name.is_empty() {
                "USB storage device"
            } else {
                &self.name
            },
            self.vendor_id,
            self.product_id,
            self.speed.label(),
            if self.behind_hub {
                ", through a hub"
            } else {
                ", direct"
            }
        )
    }
}

pub trait Hardware {
    type Transport: ScsiTransport;

    /// USB mass-storage devices currently connected.
    fn candidates(&mut self) -> Result<Vec<Candidate>>;
    /// Volumes of `disk` that are currently mounted.
    fn mounted_volumes(&mut self, disk: &str) -> Result<Vec<String>>;
    /// Unmounts every volume of `disk` so the system driver can be detached safely.
    fn eject(&mut self, disk: &str) -> Result<()>;
    /// Takes exclusive control of the device (detaching the system driver).
    fn open(&mut self, device: &Candidate) -> Result<Self::Transport>;
    /// Gives the device back to the system driver.
    fn close(&mut self, transport: Self::Transport);
    /// Asks the adapter's USB 3 hub to disable `SuperSpeed` on its port, so it reconnects over USB 2.0.
    /// The hub keeps that port at USB 2.0 (even across unplugs) until [`Hardware::restore_usb3`] is called.
    /// Changes nothing on the adapter itself.
    fn switch_to_usb2(&mut self, device: &Candidate) -> Result<()>;
    /// Re-enables `SuperSpeed` on a hub port previously switched with [`Hardware::switch_to_usb2`].
    fn restore_usb3(&mut self, hub_port: &(String, u8)) -> Result<()>;
    /// Called right after the user is asked to unplug and replug the adapter. Only the simulation acts on it.
    fn replug_requested(&mut self) {}
}

/// A simulated adapter for `--simulate` and for end-to-end tests of the wizard.
pub mod simulated {
    use super::{Candidate, Hardware, LinkSpeed};
    use crate::error::Result;
    use crate::firmware::layout;
    use crate::nvram::{USB2_ONLY_MASK, USB2_ONLY_OFFSET};
    use crate::sim::{Simulator, fixtures};

    #[allow(clippy::struct_excessive_bools)] // plain switches of a test double
    pub struct SimHardware {
        pub sim: Simulator,
        pub connected: bool,
        pub through_hub: bool,
        /// The link trains at USB 3 unless the adapter is in USB 2.0-only mode.
        pub usb3_link: bool,
        /// True while the simulated hub port has `SuperSpeed` disabled.
        pub hub_port_usb2: bool,
        pub mounted: bool,
        /// Each call to `candidates` while disconnected counts down; at zero the device "reappears".
        pub replug_after: u32,
        /// Bumped on every simulated reconnection, like a real USB address.
        pub address: u8,
        /// Simulates a user who replugs before pressing Enter: the device never appears to vanish.
        pub fast_replug: bool,
    }

    impl Default for SimHardware {
        fn default() -> Self {
            Self::new(Simulator::new(fixtures::flash(
                &fixtures::code(3),
                &fixtures::nvram(false),
            )))
        }
    }

    impl SimHardware {
        #[must_use]
        pub fn new(sim: Simulator) -> Self {
            Self {
                sim,
                connected: true,
                through_hub: true,
                usb3_link: false,
                hub_port_usb2: false,
                mounted: true,
                replug_after: 0,
                address: 1,
                fast_replug: false,
            }
        }

        fn usb2_only(&self) -> bool {
            self.sim.flash_contents()[layout::NVRAM.start + USB2_ONLY_OFFSET] & USB2_ONLY_MASK != 0
        }

        /// Simulates the user unplugging the adapter and plugging it in directly.
        pub fn simulate_replug(&mut self) {
            self.through_hub = false;
            self.usb3_link = true;
            if self.fast_replug {
                self.address += 1;
                self.mounted = true;
            } else {
                self.connected = false;
                self.replug_after = 1;
            }
        }
    }

    impl Hardware for SimHardware {
        type Transport = Simulator;

        fn candidates(&mut self) -> Result<Vec<Candidate>> {
            if !self.connected {
                if self.replug_after == 0 {
                    self.connected = true;
                    self.mounted = true;
                    self.address += 1;
                } else {
                    self.replug_after -= 1;
                }
                return Ok(Vec::new());
            }
            let usb2 = self.usb2_only();
            let speed = if usb2 || !self.usb3_link {
                LinkSpeed::High
            } else {
                LinkSpeed::Super
            };
            Ok(vec![Candidate {
                port: "1-1".into(),
                address: self.address,
                vendor_id: 0x7825,
                product_id: 0xA2A4,
                name: "Bridge01 (simulated)".into(),
                usb_version: if usb2 { 0x0200 } else { 0x0300 },
                speed,
                behind_hub: self.through_hub,
                usb3_hub_port: (self.through_hub && speed == LinkSpeed::Super).then(|| ("1".to_string(), 1)),
                disks: vec!["disk9".into()],
            }])
        }

        fn mounted_volumes(&mut self, _disk: &str) -> Result<Vec<String>> {
            Ok(if self.mounted {
                vec!["/Volumes/Simulated".into()]
            } else {
                Vec::new()
            })
        }

        fn eject(&mut self, _disk: &str) -> Result<()> {
            self.mounted = false;
            Ok(())
        }

        fn open(&mut self, _device: &Candidate) -> Result<Simulator> {
            Ok(self.sim.clone())
        }

        fn close(&mut self, _transport: Simulator) {}

        fn switch_to_usb2(&mut self, _device: &Candidate) -> Result<()> {
            if self.through_hub {
                self.usb3_link = false;
                self.hub_port_usb2 = true;
            }
            Ok(())
        }

        fn restore_usb3(&mut self, _hub_port: &(String, u8)) -> Result<()> {
            self.hub_port_usb2 = false;
            Ok(())
        }

        fn replug_requested(&mut self) {
            self.simulate_replug();
        }
    }
}
