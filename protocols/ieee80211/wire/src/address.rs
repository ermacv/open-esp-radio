//! Address roles validated before encoding an individual station frame.

use oer_protocols_ieee802_wire::{Eui48, IndividualAddress};

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

    /// The address, of either kind.
    pub const fn get(self) -> Eui48 {
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
        match address.get().octets() {
            [0, 0, 0, 0, 0, 0] => None,
            _ => Some(Self(address)),
        }
    }

    /// The individual address with its group-bit proof.
    pub const fn individual(self) -> IndividualAddress {
        self.0
    }

    /// The address, of either kind.
    pub const fn get(self) -> Eui48 {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bssid_proves_both_individual_and_nonzero() {
        assert_eq!(
            Bssid::new(IndividualAddress::new(Eui48::new([0; 6])).unwrap()),
            None
        );
        assert_eq!(IndividualAddress::new(Eui48::new([0xff; 6])), None);
        for index in 0..6 {
            let mut address = [0; 6];
            address[index] = 2;
            let bssid = Bssid::new(IndividualAddress::new(Eui48::new(address)).unwrap()).unwrap();
            assert_eq!(bssid.get().octets(), address);
            assert_eq!(bssid.individual().get().octets(), address);
        }
    }
}
