use std::{collections::HashMap, fmt::Debug, time::Duration};

use async_io::{Timer, block_on};
use futures_lite::FutureExt;
use nusb::{
    transfer::{Control, ControlType, Direction, EndpointType, Recipient, RequestBuffer}, Device, DeviceInfo, Interface
};

use super::super::{ADBMessageTransport, ADBTransport};
use crate::{
    Result, RustADBError,
    device::{ADBTransportMessage, ADBTransportMessageHeader, MessageCommand},
};

#[derive(Clone)]
struct Endpoint {
    iface: Interface,
    iface_num: u8,
    address: u8,
}

#[derive(Debug, Clone)]
struct EndpointDesc {
    iface: u8,
    address: u8,
}

/// Transport running on USB
#[derive(Clone)]
pub struct USBTransport {
    device_info: DeviceInfo,
    device: Option<Device>,
    read_endpoint: Option<Endpoint>,
    write_endpoint: Option<Endpoint>,
    other_interfaces: HashMap<u8, Interface>,
}

impl USBTransport {
    /// Instantiate a new [`USBTransport`].
    /// Only the first device with given vendor_id and product_id is returned.
    pub fn new(vendor_id: u16, product_id: u16) -> Result<Self> {
        for device_info in nusb::list_devices()? {
            if device_info.vendor_id() == vendor_id && device_info.product_id() == product_id {
                return Ok(Self::new_from_device_info(device_info));
            }
        }

        Err(RustADBError::DeviceNotFound(format!(
            "cannot find USB device with vendor_id={} and product_id={}",
            vendor_id, product_id
        )))
    }

    /// Instantiate a new [`USBTransport`] from a [`rusb::Device`].
    ///
    /// Devices can be enumerated using [`nusb::list_devices()`] and then filtered out to get desired device.
    pub fn new_from_device_info(nusb_device_info: DeviceInfo) -> Self {
        Self {
            device_info: nusb_device_info,
            device: None,
            read_endpoint: None,
            write_endpoint: None,
            other_interfaces: HashMap::new(),
        }
    }

    /// Claim a USB interface if it is not already claimed by the read or write endpoint.
    pub fn claim_interface(&mut self, iface: u8) -> Result<()> {
        if self
            .read_endpoint
            .as_ref()
            .is_some_and(|ep| ep.iface_num == iface)
            || self
                .write_endpoint
                .as_ref()
                .is_some_and(|ep| ep.iface_num == iface)
            || self.other_interfaces.get(&iface).is_some()
        {
            // Already claimed
            return Ok(());
        }

        let device = self.get_device()?;
        let interface = device.claim_interface(iface)?;
        self.other_interfaces.insert(iface, interface);
        Ok(())
    }

    /// Send a raw USB control message to the desired interface
    pub fn send_usb_class_control_msg(
        &self,
        iface: u8,
        request: u8,
        value: u16,
        index: u16,
        buf: &[u8],
        timeout: Duration,
    ) -> Result<()> {
        let interface = self.get_interface(iface)?;

        let control = Control {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request,
            value,
            index,
        };

        interface.control_out_blocking(control, buf, timeout)?;
        Ok(())
    }

    /// Perform a raw USB bulk read.
    pub fn usb_bulk_read(
        &self,
        iface: u8,
        endpoint: u8,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Result<usize> {
        let interface = self.get_interface(iface)?;
        Ok(interface.read_bulk(endpoint, buf, timeout)?)
    }

    /// Perform a raw USB bulk write.
    pub fn usb_bulk_write(
        &self,
        iface: u8,
        endpoint: u8,
        buf: &[u8],
        timeout: Duration,
    ) -> Result<usize> {
        let interface = self.get_interface(iface)?;
        Ok(interface.write_bulk(endpoint, buf, timeout)?)
    }

    fn get_device(&self) -> Result<Device> {
        self.device
            .as_ref()
            .ok_or(RustADBError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no device setup",
            )))
            .cloned()
    }

    fn get_read_endpoint(&self) -> Result<Endpoint> {
        self.read_endpoint
            .as_ref()
            .ok_or(RustADBError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no read endpoint setup",
            )))
            .cloned()
    }

    fn get_write_endpoint(&self) -> Result<&Endpoint> {
        self.write_endpoint
            .as_ref()
            .ok_or(RustADBError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no write endpoint setup",
            )))
    }

    fn get_interface(&self, iface: u8) -> Result<Interface> {
        if let Some(ep) = self.read_endpoint.as_ref() {
            if ep.iface_num == iface {
                return Ok(ep.iface.clone());
            }
        }
        if let Some(ep) = self.write_endpoint.as_ref() {
            if ep.iface_num == iface {
                return Ok(ep.iface.clone());
            }
        }
        self.other_interfaces
            .get(&iface)
            .ok_or(RustADBError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "interface not yet claimed",
            )))
            .cloned()
    }

    fn configure_endpoint(device: &Device, endpoint_desc: &EndpointDesc) -> Result<Endpoint> {
        let iface = device.claim_interface(endpoint_desc.iface)?;
        Ok(Endpoint {
            iface,
            iface_num: endpoint_desc.iface,
            address: endpoint_desc.address,
        })
    }

    fn find_endpoints(&self, device: &Device) -> Result<(EndpointDesc, EndpointDesc)> {
        let mut read_endpoint: Option<EndpointDesc> = None;
        let mut write_endpoint: Option<EndpointDesc> = None;

        for config_desc in device.configurations() {
            for interface in config_desc.interfaces() {
                for interface_desc in interface.alt_settings() {
                    for endpoint_desc in interface_desc.endpoints() {
                        if endpoint_desc.transfer_type() == EndpointType::Bulk
                            && interface_desc.class() == 0xff
                            && interface_desc.subclass() == 0x42
                            && interface_desc.protocol() == 0x01
                        {
                            let endpoint = EndpointDesc {
                                iface: interface_desc.interface_number(),
                                address: endpoint_desc.address(),
                            };
                            match endpoint_desc.direction() {
                                Direction::In => {
                                    if let Some(write_endpoint) = write_endpoint {
                                        return Ok((endpoint, write_endpoint));
                                    } else {
                                        read_endpoint = Some(endpoint);
                                    }
                                }
                                Direction::Out => {
                                    if let Some(read_endpoint) = read_endpoint {
                                        return Ok((read_endpoint, endpoint));
                                    } else {
                                        write_endpoint = Some(endpoint);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(RustADBError::USBNoDescriptorFound)
    }
}

impl ADBTransport for USBTransport {
    fn connect(&mut self) -> crate::Result<()> {
        let device = self.device_info.open()?;

        let (read_endpoint, write_endpoint) = self.find_endpoints(&device)?;

        self.read_endpoint = Some(Self::configure_endpoint(&device, &read_endpoint)?);

        self.write_endpoint = Some(Self::configure_endpoint(&device, &write_endpoint)?);

        self.device = Some(device);

        Ok(())
    }

    fn disconnect(&mut self) -> crate::Result<()> {
        let message = ADBTransportMessage::new(MessageCommand::Clse, 0, 0, &[]);
        self.write_message(message)
    }
}

impl ADBMessageTransport for USBTransport {
    fn write_message_with_timeout(
        &mut self,
        message: ADBTransportMessage,
        timeout: Duration,
    ) -> Result<()> {
        let endpoint = self.get_write_endpoint()?;

        let message_bytes = message.header().as_bytes()?;
        let mut total_written = 0;
        loop {
            total_written += endpoint.write_bulk(&message_bytes[total_written..], timeout)?;
            if total_written == message_bytes.len() {
                break;
            }
        }

        let payload = message.into_payload();
        if !payload.is_empty() {
            let mut total_written = 0;
            loop {
                total_written += endpoint.write_bulk(&payload[total_written..], timeout)?;
                if total_written == payload.len() {
                    break;
                }
            }
        }

        Ok(())
    }

    fn read_message_with_timeout(&mut self, timeout: Duration) -> Result<ADBTransportMessage> {
        let endpoint = self.get_read_endpoint()?;

        let mut data = [0; 24];
        let mut total_read = 0;
        loop {
            total_read += endpoint.read_bulk(&mut data[total_read..], timeout)?;
            if total_read == data.len() {
                break;
            }
        }

        let header = ADBTransportMessageHeader::try_from(data)?;

        log::trace!("received header {header:?}");

        if header.data_length() != 0 {
            let mut msg_data = vec![0_u8; header.data_length() as usize];
            let mut total_read = 0;
            loop {
                total_read += endpoint.read_bulk(&mut msg_data[total_read..], timeout)?;
                if total_read == msg_data.capacity() {
                    break;
                }
            }

            let message = ADBTransportMessage::from_header_and_payload(header, msg_data);

            // Check message integrity
            if !message.check_message_integrity() {
                return Err(RustADBError::InvalidIntegrity(
                    ADBTransportMessageHeader::compute_crc32(message.payload()),
                    message.header().data_crc32(),
                ));
            }

            return Ok(message);
        }

        Ok(ADBTransportMessage::from_header_and_payload(header, vec![]))
    }
}

trait InterfaceHelpers {
    fn write_bulk(&self, address: u8, buf: &[u8], timeout: Duration) -> Result<usize>;
    fn read_bulk(&self, address: u8, buf: &mut [u8], timeout: Duration) -> Result<usize>;
}

impl InterfaceHelpers for Interface {
    fn write_bulk(&self, address: u8, buf: &[u8], timeout: Duration) -> Result<usize> {
        let fut = async {
            let comp = self.bulk_out(address, buf.to_vec()).await;
            comp.status?;

            let n = comp.data.actual_length();
            Ok(n)
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
        }))
    }

    fn read_bulk(&self, address: u8, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let fut = async {
            let comp = self.bulk_in(address, RequestBuffer::new(buf.len())).await;
            comp.status?;

            let n = comp.data.len();
            buf[..n].copy_from_slice(&comp.data);
            Ok(n)
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
        }))
    }
}

impl Endpoint {
    fn write_bulk(&self, buf: &[u8], timeout: Duration) -> Result<usize> {
        self.iface.write_bulk(self.address, buf, timeout)
    }

    fn read_bulk(&self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        self.iface.read_bulk(self.address, buf, timeout)
    }
}

impl Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("iface", &self.iface.interface_number())
            .field("address", &self.address)
            .finish()
    }
}

impl Debug for USBTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("USBTransport")
            .field("device_info", &self.device_info)
            .field("read_endpoint", &self.read_endpoint)
            .field("write_endpoint", &self.write_endpoint)
            .finish()
    }
}

/// Search for adb devices with known interface class and subclass values
pub fn search_adb_devices() -> Result<Option<(u16, u16)>> {
    let mut found_devices = vec![];
    for device_info in nusb::list_devices()? {
        let Ok(device) = device_info.open() else {
            continue;
        };
        if is_adb_device(&device) {
            log::debug!(
                "Autodetect device {:04x}:{:04x}",
                device_info.vendor_id(),
                device_info.product_id()
            );
            found_devices.push((device_info.vendor_id(), device_info.product_id()));
        }
    }

    match (found_devices.first(), found_devices.get(1)) {
        (None, _) => Ok(None),
        (Some(identifiers), None) => Ok(Some(*identifiers)),
        (Some((vid1, pid1)), Some((vid2, pid2))) => Err(RustADBError::DeviceNotFound(format!(
            "Found two Android devices {:04x}:{:04x} and {:04x}:{:04x}",
            vid1, pid1, vid2, pid2
        ))),
    }
}

fn is_adb_device(device: &Device) -> bool {
    const ADB_SUBCLASS: u8 = 0x42;
    const ADB_PROTOCOL: u8 = 0x1;

    // Some devices require choosing the file transfer mode
    // for usb debugging to take effect.
    const BULK_CLASS: u8 = 0xdc;
    const BULK_ADB_SUBCLASS: u8 = 2;

    for config_desc in device.configurations() {
        for interface in config_desc.interfaces() {
            for interface_desc in interface.alt_settings() {
                let proto = interface_desc.protocol();
                let class = interface_desc.class();
                let subcl = interface_desc.subclass();
                if proto == ADB_PROTOCOL
                    && ((class == 0xff && subcl == ADB_SUBCLASS)
                        || (class == BULK_CLASS && subcl == BULK_ADB_SUBCLASS))
                {
                    return true;
                }
            }
        }
    }
    false
}
