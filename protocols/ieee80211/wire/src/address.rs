//! Address roles validated before encoding an individual station frame.

/// The receiver of an individual frame: its address has the group bit
/// clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualAddress([u8; 6]);

impl IndividualAddress {
    /// `address`, when its group bit is clear.
    pub const fn new(address: [u8; 6]) -> Option<Self> {
        if address[0] & 1 == 0 {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The address.
    pub const fn get(self) -> [u8; 6] {
        self.0
    }
}

/// The individual address of the local interface.
/// This role cannot be substituted for the other address in a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnAddress(IndividualAddress);

impl OwnAddress {
    /// Assign the role to a validated individual address.
    pub const fn new(address: IndividualAddress) -> Self {
        Self(address)
    }

    /// The individual address with its group-bit proof.
    pub const fn individual(self) -> IndividualAddress {
        self.0
    }

    /// The wire octets.
    pub const fn get(self) -> [u8; 6] {
        self.0.get()
    }
}

/// The nonzero individual address identifying an infrastructure BSS.
/// This role cannot be substituted for the other address in a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bssid(IndividualAddress);

impl Bssid {
    /// Assign the role when the individual address is nonzero. An empty scan
    /// slot has no BSSID; zero cannot stand in for an infrastructure BSS.
    pub const fn new(address: IndividualAddress) -> Option<Self> {
        match address.get() {
            [0, 0, 0, 0, 0, 0] => None,
            _ => Some(Self(address)),
        }
    }

    /// The individual address with its group-bit proof.
    pub const fn individual(self) -> IndividualAddress {
        self.0
    }

    /// The wire octets.
    pub const fn get(self) -> [u8; 6] {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bssid_proves_both_individual_and_nonzero() {
        assert_eq!(Bssid::new(IndividualAddress::new([0; 6]).unwrap()), None);
        assert_eq!(IndividualAddress::new([0xff; 6]), None);
        for index in 0..6 {
            let mut address = [0; 6];
            address[index] = 2;
            let bssid = Bssid::new(IndividualAddress::new(address).unwrap()).unwrap();
            assert_eq!(bssid.get(), address);
            assert_eq!(bssid.individual().get(), address);
        }
    }
}
