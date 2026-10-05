//! Real hardware: libusb device access and a Bulk-Only Transport.

use crate::bot;
use crate::bridge::ScsiTransport;
use crate::error::{Error, Result};
use crate::hw::{Candidate, Hardware, LinkSpeed};
use crate::os;
use rusb::{Context, Device, DeviceHandle, Direction, TransferType, UsbContext};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);
const MASS_STORAGE: u8 = 0x08;
const SCSI: u8 = 0x06;
const BULK_ONLY: u8 = 0x50;
const HUB_CLASS: u8 = 0x09;
const PORT_LINK_STATE: u16 = 5;
const SS_DISABLED: u8 = 4;
const RX_DETECT: u8 = 5;

fn is_usb3_hub(device: &Device<Context>) -> bool {
    device
        .device_descriptor()
        .is_ok_and(|d| d.class_code() == HUB_CLASS && d.usb_version() >= rusb::Version(3, 0, 0))
}

pub struct UsbHardware {
    context: Context,
}

impl UsbHardware {
    pub fn new() -> Result<Self> {
        Ok(Self {
            context: Context::new()?,
        })
    }

    fn port_path(device: &Device<Context>) -> String {
        let ports = device.port_numbers().unwrap_or_default();
        let path = ports
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(".");
        format!("{}-{path}", device.bus_number())
    }

    /// `SetPortFeature(PORT_LINK_STATE)` on a USB 3 hub port (USB 3.2 specification, 10.16.2.10).
    fn set_port_link_state(&self, (hub_port, port): &(String, u8), state: u8) -> Result<()> {
        let handle = self.find(hub_port)?.open()?;
        handle.write_control(
            0x23,
            0x03,
            PORT_LINK_STATE,
            (u16::from(state) << 8) | u16::from(*port),
            &[],
            TIMEOUT,
        )?;
        Ok(())
    }

    fn find(&self, port: &str) -> Result<Device<Context>> {
        self.context
            .devices()?
            .iter()
            .find(|d| Self::port_path(d) == port)
            .ok_or_else(|| Error::Transport("the adapter was disconnected".into()))
    }
}

/// Interface number and alternate setting implementing SCSI over Bulk-Only, if any.
fn bulk_only_interface(device: &Device<Context>) -> Option<(u8, u8, u8, u8)> {
    let config = device.active_config_descriptor().ok()?;
    for interface in config.interfaces() {
        for alt in interface.descriptors() {
            if alt.class_code() != MASS_STORAGE
                || alt.sub_class_code() != SCSI
                || alt.protocol_code() != BULK_ONLY
            {
                continue;
            }
            let bulk = |dir| {
                alt.endpoint_descriptors()
                    .find(|e| e.transfer_type() == TransferType::Bulk && e.direction() == dir)
                    .map(|e| e.address())
            };
            if let (Some(ep_in), Some(ep_out)) = (bulk(Direction::In), bulk(Direction::Out)) {
                return Some((interface.number(), alt.setting_number(), ep_in, ep_out));
            }
        }
    }
    None
}

impl Hardware for UsbHardware {
    type Transport = UsbTransport;

    fn candidates(&mut self) -> Result<Vec<Candidate>> {
        let mut found = Vec::new();
        for device in self.context.devices()?.iter() {
            if bulk_only_interface(&device).is_none() {
                continue;
            }
            let Ok(desc) = device.device_descriptor() else {
                continue;
            };
            let name = device
                .open()
                .ok()
                .and_then(|h| {
                    desc.product_string_index()
                        .and_then(|i| h.read_string_descriptor_ascii(i).ok())
                })
                .unwrap_or_default();
            let version = desc.usb_version();
            let port = Self::port_path(&device);
            let speed = match device.speed() {
                rusb::Speed::Low => LinkSpeed::Low,
                rusb::Speed::Full => LinkSpeed::Full,
                rusb::Speed::High => LinkSpeed::High,
                rusb::Speed::Super => LinkSpeed::Super,
                rusb::Speed::SuperPlus => LinkSpeed::SuperPlus,
                _ => LinkSpeed::Unknown,
            };
            let usb3_hub_port = if matches!(speed, LinkSpeed::Super | LinkSpeed::SuperPlus) {
                device
                    .get_parent()
                    .filter(is_usb3_hub)
                    .map(|hub| (Self::port_path(&hub), device.port_number()))
            } else {
                None
            };
            found.push(Candidate {
                address: device.address(),
                disks: os::disks_for_usb_device(&port, desc.vendor_id(), desc.product_id()),
                port,
                vendor_id: desc.vendor_id(),
                product_id: desc.product_id(),
                name,
                usb_version: u16::from(version.major()) << 8
                    | u16::from(version.minor()) << 4
                    | u16::from(version.sub_minor()),
                speed,
                usb3_hub_port,
                behind_hub: device.port_numbers().is_ok_and(|p| p.len() > 1),
            });
        }
        Ok(found)
    }

    fn mounted_volumes(&mut self, disk: &str) -> Result<Vec<String>> {
        Ok(os::mounted_volumes(disk))
    }

    fn eject(&mut self, disk: &str) -> Result<()> {
        os::eject(disk)
    }

    fn open(&mut self, candidate: &Candidate) -> Result<UsbTransport> {
        let device = self.find(&candidate.port)?;
        let (interface, alt, ep_in, ep_out) = bulk_only_interface(&device)
            .ok_or_else(|| Error::Transport("no Bulk-Only mass-storage interface".into()))?;
        let handle = device.open()?;
        if handle.kernel_driver_active(interface).unwrap_or(false) {
            handle.detach_kernel_driver(interface).map_err(|e| {
                Error::Transport(format!(
                    "could not take over the device from the system driver ({e}); run with sudo"
                ))
            })?;
        }
        handle.claim_interface(interface)?;
        handle.set_alternate_setting(interface, alt)?;
        Ok(UsbTransport {
            handle,
            interface,
            ep_in,
            ep_out,
            tag: 0,
        })
    }

    fn switch_to_usb2(&mut self, candidate: &Candidate) -> Result<()> {
        let hub_port = candidate
            .usb3_hub_port
            .clone()
            .ok_or_else(|| Error::Transport("the adapter is not behind a USB 3 hub".into()))?;
        // SS.Disabled: the hub stops offering SuperSpeed on this port and the device reconnects on its
        // USB 2.0 side.
        self.set_port_link_state(&hub_port, SS_DISABLED)
    }

    fn restore_usb3(&mut self, hub_port: &(String, u8)) -> Result<()> {
        // Rx.Detect: the normal idle state of a SuperSpeed port, waiting for a device.
        self.set_port_link_state(hub_port, RX_DETECT)
    }

    fn close(&mut self, transport: UsbTransport) {
        let UsbTransport {
            handle, interface, ..
        } = transport;
        let _ = handle.release_interface(interface);
        let _ = handle.attach_kernel_driver(interface);
    }
}

pub struct UsbTransport {
    handle: DeviceHandle<Context>,
    interface: u8,
    ep_in: u8,
    ep_out: u8,
    tag: u32,
}

impl UsbTransport {
    fn run(&mut self, cdb: &[u8], data_in: Option<&mut Vec<u8>>, data_out: &[u8]) -> Result<()> {
        self.tag = self.tag.wrapping_add(1);
        let tag = self.tag;
        let reading = data_in.is_some();
        let length = data_in.as_ref().map_or(data_out.len(), |b| b.len());
        let cbw = bot::command_wrapper(tag, cdb, length as u32, reading);
        let result = (|| -> Result<()> {
            self.handle.write_bulk(self.ep_out, &cbw, TIMEOUT)?;
            if let Some(buffer) = data_in {
                if !buffer.is_empty() {
                    let n = match self.handle.read_bulk(self.ep_in, buffer, TIMEOUT) {
                        Err(rusb::Error::Pipe) => {
                            self.handle.clear_halt(self.ep_in)?;
                            0
                        }
                        other => other?,
                    };
                    buffer.truncate(n);
                }
            } else if !data_out.is_empty() {
                self.handle.write_bulk(self.ep_out, data_out, TIMEOUT)?;
            }
            let mut status_wrapper = [0u8; bot::CSW_LEN];
            let n = match self.handle.read_bulk(self.ep_in, &mut status_wrapper, TIMEOUT) {
                Err(rusb::Error::Pipe) => {
                    self.handle.clear_halt(self.ep_in)?;
                    self.handle.read_bulk(self.ep_in, &mut status_wrapper, TIMEOUT)?
                }
                other => other?,
            };
            let status = bot::parse_status(&status_wrapper[..n], tag)?;
            if status.status != 0 {
                return Err(Error::CommandFailed {
                    opcode: cdb[0],
                    status: status.status,
                });
            }
            Ok(())
        })();
        if matches!(result, Err(Error::Transport(_) | Error::BadStatus(_))) {
            self.reset_recovery();
        }
        result
    }

    /// Bulk-Only Mass Storage Reset followed by clearing both halts (BOT 5.3.4).
    fn reset_recovery(&mut self) {
        let _ = self
            .handle
            .write_control(0x21, 0xFF, 0, u16::from(self.interface), &[], TIMEOUT);
        let _ = self.handle.clear_halt(self.ep_in);
        let _ = self.handle.clear_halt(self.ep_out);
    }
}

impl ScsiTransport for UsbTransport {
    fn command_in(&mut self, cdb: &[u8], len: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; len];
        self.run(cdb, Some(&mut buffer), &[])?;
        Ok(buffer)
    }

    fn command_out(&mut self, cdb: &[u8], data: &[u8]) -> Result<()> {
        self.run(cdb, None, data)
    }
}
