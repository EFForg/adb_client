#![crate_type = "lib"]
#![forbid(unsafe_code)]
#![forbid(missing_debug_implementations)]
#![forbid(missing_docs)]
#![doc = include_str!("../README.md")]

mod adb_device_ext;
mod constants;
#[cfg(any(feature = "tcp", feature = "usb"))]
mod device;
#[cfg(feature = "tcp")]
mod emulator_device;
mod error;
mod mdns;
mod models;
#[cfg(feature = "tcp")]
mod server;
#[cfg(feature = "tcp")]
mod server_device;
#[cfg(any(feature = "tcp", feature = "usb"))]
mod transports;
#[cfg(any(feature = "tcp", feature = "usb"))]
mod utils;

pub use adb_device_ext::ADBDeviceExt;
#[cfg(feature = "tcp")]
pub use device::ADBTcpDevice;
#[cfg(feature = "usb")]
pub use device::ADBUSBDevice;
#[cfg(feature = "tcp")]
pub use emulator_device::ADBEmulatorDevice;
pub use error::{Result, RustADBError};
pub use mdns::*;
pub use models::{AdbStatResponse, RebootType};
#[cfg(feature = "tcp")]
pub use server::*;
#[cfg(feature = "tcp")]
pub use server_device::ADBServerDevice;
#[cfg(any(feature = "tcp", feature = "usb"))]
pub use transports::*;
