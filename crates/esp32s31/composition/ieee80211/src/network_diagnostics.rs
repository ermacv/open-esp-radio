//! Interface selection for independent STA/AP resource observations.

/// Logical Wi-Fi endpoint whose network resources are being observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkInterface {
    Station,
    AccessPoint,
}
