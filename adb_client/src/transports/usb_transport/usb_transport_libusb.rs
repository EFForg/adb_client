use std::{sync::Arc, time::Duration};

use rusb::{
    constants::LIBUSB_CLASS_VENDOR_SPEC, Device, DeviceDescriptor, DeviceHandle, Direction, GlobalContext, TransferType, UsbContext
};

use super::super::{ADBMessageTransport, ADBTransport};
use crate::{
    Result, RustADBError,
    device::{ADBTransportMessage, ADBTransportMessageHeader, MessageCommand},
};

#[derive(Clone, Debug)]
struct Endpoint {
    iface: u8,
    address: u8,
}

/// Transport running on USB
#[derive(Debug, Clone)]
pub struct USBTransport {
    device: Device<GlobalContext>,
    handle: Option<Arc<DeviceHandle<GlobalContext>>>,
    read_endpoint: Option<Endpoint>,
    write_endpoint: Option<Endpoint>,
}

impl USBTransport {
    /// Instantiate a new [`USBTransport`].
    /// Only the first device with given vendor_id and product_id is returned.
    pub fn new(vendor_id: u16, product_id: u16) -> Result<Self> {
        for device in rusb::devices()?.iter() {
            if let Ok(descriptor) = device.device_descriptor() {
                if descriptor.vendor_id() == vendor_id && descriptor.product_id() == product_id {
                    return Ok(Self::new_from_device(device));
                }
            }
        }

        Err(RustADBError::DeviceNotFound(format!(
            "cannot find USB device with vendor_id={} and product_id={}",
            vendor_id, product_id
        )))
    }

    /// Instantiate a new [`USBTransport`] from a [`rusb::Device`].
    ///
    /// Devices can be enumerated using [`rusb::devices()`] and then filtered out to get desired device.
    pub fn new_from_device(rusb_device: rusb::Device<GlobalContext>) -> Self {
        Self {
            device: rusb_device,
            handle: None,
            read_endpoint: None,
            write_endpoint: None,
        }
    }

    pub(crate) fn get_raw_connection(&self) -> Result<Arc<DeviceHandle<GlobalContext>>> {
        self.handle
            .as_ref()
            .ok_or(RustADBError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "not connected",
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

    fn configure_endpoint(handle: &DeviceHandle<GlobalContext>, endpoint: &Endpoint) -> Result<()> {
        handle.claim_interface(endpoint.iface)?;
        Ok(())
    }

    fn find_endpoints(&self, handle: &DeviceHandle<GlobalContext>) -> Result<(Endpoint, Endpoint)> {
        let mut read_endpoint: Option<Endpoint> = None;
        let mut write_endpoint: Option<Endpoint> = None;

        for n in 0..handle.device().device_descriptor()?.num_configurations() {
            let config_desc = match handle.device().config_descriptor(n) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for interface in config_desc.interfaces() {
                for interface_desc in interface.descriptors() {
                    for endpoint_desc in interface_desc.endpoint_descriptors() {
                        if endpoint_desc.transfer_type() == TransferType::Bulk
                            && interface_desc.class_code() == LIBUSB_CLASS_VENDOR_SPEC
                            && interface_desc.sub_class_code() == 0x42
                            && interface_desc.protocol_code() == 0x01
                        {
                            let endpoint = Endpoint {
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
        let device = self.device.open()?;

        let (read_endpoint, write_endpoint) = self.find_endpoints(&device)?;

        Self::configure_endpoint(&device, &read_endpoint)?;
        self.read_endpoint = Some(read_endpoint);

        Self::configure_endpoint(&device, &write_endpoint)?;
        self.write_endpoint = Some(write_endpoint);

        self.handle = Some(Arc::new(device));

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
        let handle = self.get_raw_connection()?;

        let message_bytes = message.header().as_bytes()?;
        let mut total_written = 0;
        loop {
            total_written +=
                handle.write_bulk(endpoint.address, &message_bytes[total_written..], timeout)?;
            if total_written == message_bytes.len() {
                break;
            }
        }

        let payload = message.into_payload();
        if !payload.is_empty() {
            let mut total_written = 0;
            loop {
                total_written +=
                    handle.write_bulk(endpoint.address, &payload[total_written..], timeout)?;
                if total_written == payload.len() {
                    break;
                }
            }
        }

        Ok(())
    }

    fn read_message_with_timeout(&mut self, timeout: Duration) -> Result<ADBTransportMessage> {
        let endpoint = self.get_read_endpoint()?;
        let handle = self.get_raw_connection()?;

        let mut data = [0; 24];
        let mut total_read = 0;
        loop {
            total_read += handle.read_bulk(endpoint.address, &mut data[total_read..], timeout)?;
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
                total_read +=
                    handle.read_bulk(endpoint.address, &mut msg_data[total_read..], timeout)?;
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

/// Search for adb devices with known interface class and subclass values
pub fn search_adb_devices() -> Result<Option<(u16, u16)>> {
    let mut found_devices = vec![];
    for device in rusb::devices()?.iter() {
        let Ok(des) = device.device_descriptor() else {
            continue;
        };
        if is_adb_device(&device, &des) {
            log::debug!(
                "Autodetect device {:04x}:{:04x}",
                des.vendor_id(),
                des.product_id()
            );
            found_devices.push((des.vendor_id(), des.product_id()));
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

fn is_adb_device<T: UsbContext>(device: &Device<T>, des: &DeviceDescriptor) -> bool {
    const ADB_SUBCLASS: u8 = 0x42;
    const ADB_PROTOCOL: u8 = 0x1;

    // Some devices require choosing the file transfer mode
    // for usb debugging to take effect.
    const BULK_CLASS: u8 = 0xdc;
    const BULK_ADB_SUBCLASS: u8 = 2;

    for n in 0..des.num_configurations() {
        let Ok(config_des) = device.config_descriptor(n) else {
            continue;
        };
        for interface in config_des.interfaces() {
            for interface_des in interface.descriptors() {
                let proto = interface_des.protocol_code();
                let class = interface_des.class_code();
                let subcl = interface_des.sub_class_code();
                if proto == ADB_PROTOCOL
                    && ((class == LIBUSB_CLASS_VENDOR_SPEC && subcl == ADB_SUBCLASS)
                        || (class == BULK_CLASS && subcl == BULK_ADB_SUBCLASS))
                {
                    return true;
                }
            }
        }
    }
    false
}
