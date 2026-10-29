//! Serial PHY byte transport over an exclusively borrowed master.
//! SOURCE: ROM_REV0_PHY_I2C, BLOB_LIBPHY_PHY_I2C.
use oer_espressif_analog::model::{ByteAddress, Host};

/// One byte has not started or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cAccessError {
    Busy,
    /// The physical byte has no admitted route in the production profile.
    UnroutedAddress,
}

/// Pure chip routing facts and single master observations.
pub trait SamplePort {
    /// Chip-validated route. A fake port's route cannot authorize a real master.
    type Address: Copy;
    fn route(&self, address: ByteAddress) -> Option<Self::Address>;
    fn host(address: Self::Address) -> Host;
    fn is_busy(&self, host: Host) -> bool;
    fn data(&self, host: Host) -> u8;
}

/// An exclusive master loan; each method publishes one sourced write image.
pub trait Port: SamplePort {
    fn configure_host_map(&mut self);
    fn publish_read_mask(&mut self, address: Self::Address);
    fn publish_command(&mut self, address: Self::Address, value: u8, write: bool);
}

/// One serial resource, retained across start and completion by its caller.
pub struct AnalogBus<P> {
    port: P,
}

impl<P: Port> AnalogBus<P> {
    pub const fn new(port: P) -> Self {
        Self { port }
    }

    /// Refuse an unrouted or busy host before the first write.
    pub fn try_start_read(&mut self, address: ByteAddress) -> Result<(), PhyI2cAccessError> {
        let route = route(&self.port, address)?;
        if self.port.is_busy(P::host(route)) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.port.configure_host_map();
        self.port.publish_read_mask(route);
        self.port.publish_command(route, 0, false);
        Ok(())
    }

    pub fn try_finish_read(&self, address: ByteAddress) -> Result<u8, PhyI2cAccessError> {
        try_finish_read(&self.port, address)
    }

    /// Writes retain the read-mask word, matching the vendor leaf.
    pub fn try_start_write(
        &mut self,
        address: ByteAddress,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let route = route(&self.port, address)?;
        if self.port.is_busy(P::host(route)) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.port.configure_host_map();
        self.port.publish_command(route, value, true);
        Ok(())
    }

    pub fn try_finish_write(&self, address: ByteAddress) -> Result<(), PhyI2cAccessError> {
        try_finish_write(&self.port, address)
    }
}

fn route<P: SamplePort>(port: &P, address: ByteAddress) -> Result<P::Address, PhyI2cAccessError> {
    port.route(address)
        .ok_or(PhyI2cAccessError::UnroutedAddress)
}

/// One busy sample, followed by a fresh data sample only on completion.
pub fn try_finish_read<P: SamplePort>(
    port: &P,
    address: ByteAddress,
) -> Result<u8, PhyI2cAccessError> {
    let host = P::host(route(port, address)?);
    if port.is_busy(host) {
        return Err(PhyI2cAccessError::Busy);
    }
    Ok(port.data(host))
}

/// One busy sample on the same address's host, with no data read.
pub fn try_finish_write<P: SamplePort>(
    port: &P,
    address: ByteAddress,
) -> Result<(), PhyI2cAccessError> {
    let host = P::host(route(port, address)?);
    if port.is_busy(host) {
        Err(PhyI2cAccessError::Busy)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
