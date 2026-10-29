//! One numeric fault envelope. Storage, election and reset belong to the platform.
//! No old-layout decoder is provided. Integrity detects damaged bytes; it does
//! not guarantee retention or an atomic commit across a loss of power.

/// Largest existing observation: the 280-byte HIL postmortem payload.
pub const PAYLOAD_BYTES: usize = 280;
pub const VERSION: u32 = 1;
/// No hart sample was supplied by a pure payload encoder. InvalidHart records
/// instead retain their exact observed value, including this bit pattern.
pub const UNKNOWN_HART: u32 = u32::MAX;
const COMPLETE: u32 = u32::from_le_bytes(*b"OERF");

/// Numeric spellings of the classes in terminal-sites.toml.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Class {
    HardwareImpossible = 1,
    UnexpectedHardware = 2,
    SoftwareInvariant = 3,
    ConfigurationBoot = 4,
    Exhaustion = 5,
    Cancellation = 6,
    CpuException = 7,
    RequestedReset = 8,
    RustPanic = 9,
}

impl Class {
    pub const fn from_code(code: u32) -> Result<Self, Error> {
        Ok(match code {
            1 => Self::HardwareImpossible,
            2 => Self::UnexpectedHardware,
            3 => Self::SoftwareInvariant,
            4 => Self::ConfigurationBoot,
            5 => Self::Exhaustion,
            6 => Self::Cancellation,
            7 => Self::CpuException,
            8 => Self::RequestedReset,
            9 => Self::RustPanic,
            _ => return Err(Error::UnknownClass(code)),
        })
    }
}

/// Diagnostic namespaces; reason numbers are local to a namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Domain {
    Runtime = 1,
    Panic = 2,
    InvalidHart = 3,
    Phy = 4,
    Wifi = 5,
    BluetoothGate = 6,
    BluetoothRelease = 7,
    Ieee802154 = 8,
    Hil = 9,
    RxChain = 10,
    Postmortem = 11,
    CpuException = 12,
    Boot = 13,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Incomplete,
    UnknownVersion(u32),
    UnknownClass(u32),
    Overflow { length: usize },
    Corrupt,
    Payload,
    Domain,
}

/// Every bit pattern is valid Rust data. Only `peek` validates a diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct Record {
    complete: u32,
    version: u32,
    length: u32,
    class: u32,
    domain: u32,
    reason: u32,
    hart: u32,
    context: u32,
    payload: [u8; PAYLOAD_BYTES],
    checksum: u32,
}

const _: () = const {
    if core::mem::size_of::<Record>() != 316 {
        panic!("fault envelope layout must match version one");
    }
};

impl Record {
    pub const EMPTY: Self = Self {
        complete: 0,
        version: VERSION,
        length: 0,
        class: 0,
        domain: 0,
        reason: 0,
        hart: 0,
        context: 0,
        payload: [0; PAYLOAD_BYTES],
        checksum: 0,
    };

    /// Fixed payload geometry is checked at monomorphization. Dynamic input
    /// uses `new`, whose oversized input returns a typed refusal.
    pub fn encode<T: bytemuck::NoUninit>(
        class: Class,
        domain: Domain,
        reason: u32,
        hart: u32,
        context: u32,
        payload: &T,
    ) -> Result<Self, Error> {
        const {
            if core::mem::size_of::<T>() > PAYLOAD_BYTES {
                panic!("fixed fault payload exceeds the shared envelope");
            }
        }
        Self::new(
            class,
            domain,
            reason,
            hart,
            context,
            bytemuck::bytes_of(payload),
        )
    }

    /// Encode a bounded payload without truncation or an implicit fallback.
    pub fn new(
        class: Class,
        domain: Domain,
        reason: u32,
        hart: u32,
        context: u32,
        payload: &[u8],
    ) -> Result<Self, Error> {
        let mut record = Self::EMPTY;
        let to = record
            .payload
            .get_mut(..payload.len())
            .ok_or(Error::Overflow {
                length: payload.len(),
            })?;
        for (to, from) in to.iter_mut().zip(payload) {
            *to = *from;
        }
        record.length = u32::try_from(payload.len()).map_err(|_| Error::Overflow {
            length: payload.len(),
        })?;
        record.class = class as u32;
        record.domain = domain as u32;
        record.reason = reason;
        record.hart = hart;
        record.context = context;
        record.checksum = record.sum();
        record.complete = COMPLETE;
        Ok(record)
    }

    fn sum(&self) -> u32 {
        let mut sum = 0x811c_9dc5_u32;
        for byte in [
            self.version,
            self.length,
            self.class,
            self.domain,
            self.reason,
            self.hart,
            self.context,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .chain(self.payload)
        {
            sum = (sum ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
        sum
    }

    pub fn peek(&self) -> Result<View<'_>, Error> {
        if self.complete != COMPLETE {
            return Err(Error::Incomplete);
        }
        if self.version != VERSION {
            return Err(Error::UnknownVersion(self.version));
        }
        let length = usize::try_from(self.length).map_err(|_| Error::Payload)?;
        let payload = self
            .payload
            .get(..length)
            .ok_or(Error::Overflow { length })?;
        let class = Class::from_code(self.class)?;
        if self.checksum != self.sum() {
            return Err(Error::Corrupt);
        }
        Ok(View {
            class,
            domain: self.domain,
            reason: self.reason,
            hart: self.hart,
            context: self.context,
            payload,
        })
    }

    /// Commit marker is written last, after all bytes and their checksum.
    pub fn commit(&mut self, mut record: Self) -> Result<(), Error> {
        record.peek()?;
        record.complete = 0;
        self.complete = 0;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        *self = record;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        self.complete = COMPLETE;
        Ok(())
    }

    /// The emitter's `Ok` acknowledges the complete output. Refusal keeps bytes.
    pub fn report<E>(
        &mut self,
        emit: impl FnOnce(View<'_>) -> Result<(), E>,
    ) -> Result<(), ReportError<E>> {
        emit(self.peek().map_err(ReportError::Record)?).map_err(ReportError::Output)?;
        self.complete = 0;
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ReportError<E> {
    Record(Error),
    Output(E),
}

#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub class: Class,
    pub domain: u32,
    pub reason: u32,
    pub hart: u32,
    pub context: u32,
    pub payload: &'a [u8],
}

impl View<'_> {
    pub fn decode<T: bytemuck::AnyBitPattern>(&self, domain: Domain) -> Result<T, Error> {
        if self.domain != domain as u32 {
            return Err(Error::Domain);
        }
        bytemuck::try_pod_read_unaligned(self.payload).map_err(|_| Error::Payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_rejected_commit_does_not_destroy_the_previous_record() {
        let mut previous = Record::new(Class::RustPanic, Domain::Panic, 1, 0, 0, &[7]).unwrap();
        let original = previous;
        let mut corrupt = original;
        corrupt.checksum ^= 1;
        assert_eq!(previous.commit(corrupt), Err(Error::Corrupt));
        assert_eq!(previous, original);
        corrupt.version = u32::MAX;
        let emitted = core::cell::Cell::new(false);
        assert_eq!(
            corrupt.report(|_| {
                emitted.set(true);
                Ok::<_, ()>(())
            }),
            Err(ReportError::Record(Error::UnknownVersion(u32::MAX)))
        );
        assert!(!emitted.get());
        assert_eq!(corrupt.version, u32::MAX);
    }
    #[test]
    fn reboot_peek_output_refusal_and_success() {
        let first = Record::new(
            Class::UnexpectedHardware,
            Domain::BluetoothGate,
            11,
            u32::MAX,
            3,
            &[0xff; 44],
        )
        .unwrap();
        let mut reboot = Record::EMPTY;
        reboot.commit(first).unwrap();
        assert_eq!(reboot.peek().unwrap().payload, &[0xff; 44]);
        assert_eq!(reboot.report(|_| Err(7)), Err(ReportError::Output(7)));
        assert_eq!(reboot, first);
        reboot
            .report(|view| {
                assert_eq!(view.reason, 11);
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(reboot.peek().unwrap_err(), Error::Incomplete);
    }
    #[test]
    fn corruption_version_and_overflow_are_typed() {
        let valid = Record::new(Class::SoftwareInvariant, Domain::Runtime, 1, 0, 0, &[7]).unwrap();
        let mut bad = valid;
        bad.payload[0] ^= 1;
        assert_eq!(bad.peek().unwrap_err(), Error::Corrupt);
        bad = valid;
        bad.version = u32::MAX;
        assert_eq!(bad.peek().unwrap_err(), Error::UnknownVersion(u32::MAX));
        bad = valid;
        bad.length = u32::MAX;
        assert!(matches!(bad.peek(), Err(Error::Overflow { .. })));
        assert!(matches!(
            Record::new(
                Class::Exhaustion,
                Domain::Runtime,
                1,
                0,
                0,
                &[0; PAYLOAD_BYTES + 1]
            ),
            Err(Error::Overflow { .. })
        ));
        bad = valid;
        bad.class = u32::MAX;
        assert_eq!(bad.peek().unwrap_err(), Error::UnknownClass(u32::MAX));
    }
}
