#[cfg(all(feature = "usb", target_os = "linux"))]
mod usb_transport_nusb;
#[cfg(all(feature = "usb", target_os = "linux"))]
pub use usb_transport_nusb::*;

#[cfg(all(feature = "usb", any(target_os = "windows", target_os = "macos")))]
mod usb_transport_libusb;
#[cfg(all(feature = "usb", any(target_os = "windows", target_os = "macos")))]
pub use usb_transport_libusb::*;
