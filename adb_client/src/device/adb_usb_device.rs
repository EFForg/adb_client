use std::fs::read_to_string;
use std::io::Read;
use std::io::Write;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use super::adb_message_device::ADBMessageDevice;
use super::get_default_adb_key_path;
use super::models::MessageCommand;
use super::{ADBRsaKey, ADBTransportMessage};
use crate::search_adb_devices;
use crate::ADBDeviceExt;
use crate::ADBMessageTransport;
use crate::ADBTransport;
use crate::device::adb_transport_message::{AUTH_RSAPUBLICKEY, AUTH_SIGNATURE, AUTH_TOKEN};
use crate::{Result, RustADBError, USBTransport};

pub fn read_adb_private_key<P: AsRef<Path>>(private_key_path: P) -> Result<Option<ADBRsaKey>> {
    let pk = match read_to_string(private_key_path.as_ref()) {
        Ok(contents) => contents,
        Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match ADBRsaKey::new_from_pkcs8(&pk) {
        Ok(pk) => Ok(Some(pk)),
        Err(e) => {
            log::error!("Error while create RSA private key: {e}");
            Ok(None)
        }
    }
}

/// Represent a device reached and available over USB.
#[derive(Debug)]
pub struct ADBUSBDevice {
    private_key: ADBRsaKey,
    inner: ADBMessageDevice<USBTransport>,
}

impl ADBUSBDevice {
    /// Instantiate a new [`ADBUSBDevice`]
    pub fn new(vendor_id: u16, product_id: u16) -> Result<Self> {
        Self::new_with_custom_private_key(vendor_id, product_id, get_default_adb_key_path()?)
    }

    /// Instantiate a new [`ADBUSBDevice`] using a custom private key path
    pub fn new_with_custom_private_key(
        vendor_id: u16,
        product_id: u16,
        private_key_path: PathBuf,
    ) -> Result<Self> {
        Self::new_from_transport_inner(USBTransport::new(vendor_id, product_id)?, private_key_path)
    }

    /// Instantiate a new [`ADBUSBDevice`] from a [`USBTransport`] and an optional private key path.
    pub fn new_from_transport(
        transport: USBTransport,
        private_key_path: Option<PathBuf>,
    ) -> Result<Self> {
        let private_key_path = match private_key_path {
            Some(private_key_path) => private_key_path,
            None => get_default_adb_key_path()?,
        };

        Self::new_from_transport_inner(transport, private_key_path)
    }

    fn new_from_transport_inner(
        transport: USBTransport,
        private_key_path: PathBuf,
    ) -> Result<Self> {
        let private_key = match read_adb_private_key(private_key_path)? {
            Some(pk) => pk,
            None => ADBRsaKey::new_random()?,
        };

        let mut s = Self {
            private_key,
            inner: ADBMessageDevice::new(transport),
        };

        s.connect()?;

        Ok(s)
    }

    /// autodetect connected ADB devices and establish a connection with the first device found
    pub fn autodetect() -> Result<Self> {
        Self::autodetect_with_custom_private_key(get_default_adb_key_path()?)
    }

    /// autodetect connected ADB devices and establish a connection with the first device found using a custom private key path
    pub fn autodetect_with_custom_private_key(private_key_path: PathBuf) -> Result<Self> {
        match search_adb_devices()? {
            Some((vendor_id, product_id)) => {
                ADBUSBDevice::new_with_custom_private_key(vendor_id, product_id, private_key_path)
            }
            _ => Err(RustADBError::DeviceNotFound(
                "cannot find USB devices matching the signature of an ADB device".into(),
            )),
        }
    }

    /// Send initial connect
    pub fn connect(&mut self) -> Result<()> {
        self.get_transport_mut().connect()?;

        let message = ADBTransportMessage::new(
            MessageCommand::Cnxn,
            0x01000000,
            1048576,
            format!("host::{}\0", env!("CARGO_PKG_NAME")).as_bytes(),
        );

        self.get_transport_mut().write_message(message)?;

        let message = self.get_transport_mut().read_message()?;
        // If the device returned CNXN instead of AUTH it does not require authentication,
        // so we can skip the auth steps.
        if message.header().command() == MessageCommand::Cnxn {
            self.inner.set_maximum_data_size(message.header().arg1())?;
            return Ok(());
        }
        message.assert_command(MessageCommand::Auth)?;

        // At this point, we should have receive an AUTH message with arg0 == 1
        let auth_message = match message.header().arg0() {
            AUTH_TOKEN => message,
            v => {
                return Err(RustADBError::ADBRequestFailed(format!(
                    "Received AUTH message with type != 1 ({v})"
                )));
            }
        };

        let sign = self.private_key.sign(auth_message.into_payload())?;

        let message = ADBTransportMessage::new(MessageCommand::Auth, AUTH_SIGNATURE, 0, &sign);

        self.get_transport_mut().write_message(message)?;

        let received_response = self.get_transport_mut().read_message()?;

        if received_response.header().command() == MessageCommand::Cnxn {
            log::info!(
                "Authentication OK, device info {}",
                String::from_utf8(received_response.into_payload())?
            );
            return Ok(());
        }

        let mut pubkey = self.private_key.android_pubkey_encode()?.into_bytes();
        pubkey.push(b'\0');

        let message = ADBTransportMessage::new(MessageCommand::Auth, AUTH_RSAPUBLICKEY, 0, &pubkey);

        self.get_transport_mut().write_message(message)?;

        let response = self
            .get_transport_mut()
            .read_message_with_timeout(Duration::from_secs(10))
            .and_then(|message| {
                message.assert_command(MessageCommand::Cnxn)?;
                self.inner.set_maximum_data_size(message.header().arg1())?;
                Ok(message)
            })?;

        log::info!(
            "Authentication OK, device info {}",
            String::from_utf8(response.into_payload())?
        );

        Ok(())
    }

    #[inline]
    /// Get a reference to the underlying [`USBTransport`].
    pub fn get_transport_mut(&mut self) -> &mut USBTransport {
        self.inner.get_transport_mut()
    }
}

impl ADBDeviceExt for ADBUSBDevice {
    #[inline]
    fn shell_command(&mut self, command: &[&str], output: &mut dyn Write) -> Result<()> {
        self.inner.shell_command(command, output)
    }

    #[inline]
    fn shell<'a>(&mut self, reader: &mut dyn Read, writer: Box<(dyn Write + Send)>) -> Result<()> {
        self.inner.shell(reader, writer)
    }

    #[inline]
    fn stat(&mut self, remote_path: &str) -> Result<crate::AdbStatResponse> {
        self.inner.stat(remote_path)
    }

    #[inline]
    fn pull(&mut self, source: &dyn AsRef<str>, output: &mut dyn Write) -> Result<()> {
        self.inner.pull(source, output)
    }

    #[inline]
    fn push(&mut self, stream: &mut dyn Read, path: &dyn AsRef<str>) -> Result<()> {
        self.inner.push(stream, path)
    }

    #[inline]
    fn reboot(&mut self, reboot_type: crate::RebootType) -> Result<()> {
        self.inner.reboot(reboot_type)
    }

    #[inline]
    fn install(&mut self, apk_path: &dyn AsRef<Path>) -> Result<()> {
        self.inner.install(apk_path)
    }

    #[inline]
    fn uninstall(&mut self, package: &str) -> Result<()> {
        self.inner.uninstall(package)
    }

    #[inline]
    fn framebuffer_inner(&mut self) -> Result<image::ImageBuffer<image::Rgba<u8>, Vec<u8>>> {
        self.inner.framebuffer_inner()
    }
}

impl Drop for ADBUSBDevice {
    fn drop(&mut self) {
        // Best effort here
        let _ = self.get_transport_mut().disconnect();
    }
}
