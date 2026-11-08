//! The load service's names the facade uses.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticAddress {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: [u8; 4],
}
pub struct Start {
    pub address: Option<StaticAddress>,
    pub port: u16,
}
#[derive(Debug)]
pub enum LoadError {
    NoAddress,
    Busy,
    InvalidParameters,
    Bind,
}
