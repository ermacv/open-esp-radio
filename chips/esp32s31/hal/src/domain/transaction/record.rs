//! Fixed-size retained diagnostic codec shared by image compositions.
use super::{Fault, FaultReason};
use core::fmt::{self, Write};
const MAGIC: u32 = 0x5054_5801;

/// Twenty-four retained bytes, with no pointers or invalid bit patterns.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct FaultRecord {
    magic: u32,
    reason: u32,
    owner_low: u32,
    owner_high: u32,
    registration: u32,
    checksum: u32,
}
impl FaultRecord {
    pub fn record(&mut self, fault: Fault) {
        self.magic = 0;
        self.reason = match fault.reason {
            FaultReason::TransactionDropped => 1,
            FaultReason::ReturnOwnerDropped => 2,
            FaultReason::IdentityClaimed => 3,
            FaultReason::OwnerUnavailable => 4,
        };
        let owner = fault.owner as u64;
        self.owner_low = owner as u32;
        self.owner_high = (owner >> 32) as u32;
        self.registration = fault.registration.map_or(0, |epoch| epoch.get());
        self.checksum = self.checksum();
        self.magic = MAGIC;
    }
    fn checksum(&self) -> u32 {
        [
            self.reason,
            self.owner_low,
            self.owner_high,
            self.registration,
        ]
        .iter()
        .fold(MAGIC, |sum, word| (sum ^ word).wrapping_mul(0x0100_0193))
    }
    /// Report a valid record; failed output retains it for the next boot.
    pub fn report(&mut self, write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
        if self.magic != MAGIC
            || self.checksum != self.checksum()
            || !(1..=4).contains(&self.reason)
        {
            return Ok(());
        }
        struct Output<F>(F);
        impl<F: FnMut(&[u8]) -> bool> Write for Output<F> {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                if (self.0)(text.as_bytes()) {
                    Ok(())
                } else {
                    Err(fmt::Error)
                }
            }
        }
        let reason = match self.reason {
            1 => "TransactionDropped",
            2 => "ReturnOwnerDropped",
            3 => "IdentityClaimed",
            4 => "OwnerUnavailable",
            _ => return Ok(()),
        };
        let owner = u64::from(self.owner_low) | (u64::from(self.owner_high) << 32);
        writeln!(
            Output(write),
            "previous PHY transaction fault reason={reason} owner={owner:#x} registration={}",
            self.registration
        )?;
        self.magic = 0;
        Ok(())
    }
}
const _: () = assert!(core::mem::size_of::<FaultRecord>() == 24);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_output_retains_full_observations_until_reported() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        let mut registration = crate::phy::registration::PhyRegistration::new();
        let epoch = registration.begin();
        record.record(Fault {
            reason: FaultReason::TransactionDropped,
            owner: usize::MAX,
            registration: Some(epoch),
        });
        assert!(record.report(|_| false).is_err());
        let mut bytes = std::vec::Vec::new();
        assert!(
            record
                .report(|chunk| {
                    bytes.extend_from_slice(chunk);
                    true
                })
                .is_ok()
        );
        let text = std::string::String::from_utf8(bytes).unwrap();
        assert!(text.contains("reason=TransactionDropped"));
        assert!(text.contains(&std::format!("owner={:#x}", usize::MAX)));
        assert!(text.contains(&std::format!("registration={}", epoch.get())));
        assert_eq!(record.magic, 0);
        assert!(record.report(|_| panic!("reported once")).is_ok());
    }
    #[test]
    fn corrupted_record_is_not_reported() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        record.record(Fault {
            reason: FaultReason::ReturnOwnerDropped,
            owner: 123,
            registration: None,
        });
        record.owner_low ^= 1;
        assert!(record.report(|_| panic!("corrupt record")).is_ok());
    }
}
