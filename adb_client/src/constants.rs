#[cfg(any(feature = "tcp", feature = "usb"))]
pub const BUFFER_SIZE: usize = 65536;
