//! The load service's names the facade uses.

pub struct Start {
    pub port: u16,
}
#[derive(Debug)]
pub enum LoadError {
    NoAddress,
    Busy,
    InvalidParameters,
    Bind,
}
