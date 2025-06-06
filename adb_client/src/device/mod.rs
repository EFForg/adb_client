mod adb_message_device;
mod adb_message_device_commands;
#[cfg(feature = "tcp")]
mod adb_tcp_device;
mod adb_transport_message;
#[cfg(feature = "usb")]
mod adb_usb_device;
mod commands;
mod message_writer;
mod models;
mod shell_message_writer;

use std::path::PathBuf;

use adb_message_device::ADBMessageDevice;
#[cfg(feature = "tcp")]
pub use adb_tcp_device::ADBTcpDevice;
#[cfg(any(feature = "tcp", feature = "usb"))]
pub use adb_transport_message::ADBTransportMessageHeader;
pub use adb_transport_message::ADBTransportMessage;
#[cfg(feature = "usb")]
pub use adb_usb_device::ADBUSBDevice;
pub use message_writer::MessageWriter;
pub use models::{MessageCommand, MessageSubcommand};
#[cfg(feature = "usb")]
pub use models::ADBRsaKey;
pub use shell_message_writer::ShellMessageWriter;

use crate::{Result, RustADBError};

pub fn get_default_adb_key_path() -> Result<PathBuf> {
    homedir::my_home()
        .ok()
        .flatten()
        .map(|home| home.join(".android").join("adbkey"))
        .ok_or(RustADBError::NoHomeDirectory)
}

