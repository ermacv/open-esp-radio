//! A failed boot check, kept for the next boot.
//!
//! A boot check that fails records its [`Failure`] and the hart in
//! `.rtc_fast.persistent` without formatting, then resets the chip; the
//! next boot's [`report_previous`] prints the record once. A CPU
//! exception's records are the runtime's trap frame and the platform's
//! observations after the trap (`oer_esp32s31_compose::exception`).

use oer_diagnostics::Note;
use oer_diagnostics::fault::{Class, Domain, Record};
use oer_espressif_staged_runtime::retained_fault::{self, Claim};
use oer_espressif_staged_runtime::trap::TrapRegisters;

/// The record's reason: a failed boot check.
const FAILURE: u32 = 3;

oer_espressif_staged_runtime::retained_static! {
    /// The failure record. The fatal path takes it without waiting: while
    /// the other hart records its own failure, this one's is skipped.
    static SLOT: Record;
}


/// A failed check of the boot or of the station's composition; it resets
/// the image.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum Failure {
    Layout = 1,
    TaskAllocation = 2,
    RootTaken = 4,
    InterruptTable = 5,
    Console = 6,
    InterruptHart = 7,
    InterruptStack = 8,
    InterruptVectors = 9,
    InterruptControlBits = 10,
    InterruptCount = 11,
    InterruptLevelBits = 12,
    InterruptStackGuard = 13,
    RootInitialization = 14,
    EntropySource = 15,
    Station = 16,
    Storage = 17,
    NonceExhausted = 18,
    Network = 19,
    Cores = 20,
}

impl Failure {
    const ALL: [Self; 19] = [
        Self::Layout,
        Self::TaskAllocation,
        Self::RootTaken,
        Self::InterruptTable,
        Self::Console,
        Self::InterruptHart,
        Self::InterruptStack,
        Self::InterruptVectors,
        Self::InterruptControlBits,
        Self::InterruptCount,
        Self::InterruptLevelBits,
        Self::InterruptStackGuard,
        Self::RootInitialization,
        Self::EntropySource,
        Self::Station,
        Self::Storage,
        Self::NonceExhausted,
        Self::Network,
        Self::Cores,
    ];

    pub(crate) fn from_boot(error: oer_espressif_staged_runtime::BootError) -> Self {
        use oer_espressif_interrupt_table::Error;
        use oer_espressif_staged_runtime::BootError;
        match error {
            BootError::WrongHart { .. } => Self::InterruptHart,
            BootError::NoInterruptStack { .. } => Self::InterruptStack,
            BootError::InvalidStackGuard { .. } => Self::InterruptStackGuard,
            BootError::ForeignVectors => Self::InterruptVectors,
            BootError::InterruptController(Error::WrongControlBits { .. }) => {
                Self::InterruptControlBits
            }
            BootError::InterruptController(Error::WrongInterruptCount { .. }) => {
                Self::InterruptCount
            }
            BootError::InterruptController(Error::WrongLevelBits { .. }) => {
                Self::InterruptLevelBits
            }
            BootError::InterruptController(_) => Self::InterruptTable,
        }
    }

    pub(crate) const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::Layout => oer_diagnostics::name!("layout"; 32),
            Self::TaskAllocation => oer_diagnostics::name!("task-allocation"; 32),
            Self::RootTaken => oer_diagnostics::name!("root-taken"; 32),
            Self::RootInitialization => oer_diagnostics::name!("root-initialization"; 32),
            Self::InterruptTable => oer_diagnostics::name!("interrupt-table"; 32),
            Self::Console => oer_diagnostics::name!("console"; 32),
            Self::InterruptHart => oer_diagnostics::name!("interrupt-hart"; 32),
            Self::InterruptStack => oer_diagnostics::name!("interrupt-stack"; 32),
            Self::InterruptStackGuard => oer_diagnostics::name!("interrupt-stack-guard"; 32),
            Self::InterruptVectors => oer_diagnostics::name!("interrupt-vectors"; 32),
            Self::InterruptControlBits => oer_diagnostics::name!("interrupt-control-bits"; 32),
            Self::InterruptCount => oer_diagnostics::name!("interrupt-count"; 32),
            Self::InterruptLevelBits => oer_diagnostics::name!("interrupt-level-bits"; 32),
            Self::EntropySource => oer_diagnostics::name!("entropy-source"; 32),
            Self::Station => oer_diagnostics::name!("station"; 32),
            Self::Storage => oer_diagnostics::name!("storage"; 32),
            Self::NonceExhausted => oer_diagnostics::name!("nonce-exhausted"; 32),
            Self::Network => oer_diagnostics::name!("network"; 32),
            Self::Cores => oer_diagnostics::name!("cores-taken"; 32),
        }
    }

    /// The fault class the record of this failure states.
    const fn class(self) -> Class {
        match self {
            Self::EntropySource => Class::UnexpectedHardware,
            Self::NonceExhausted => Class::Exhaustion,
            Self::Network => Class::SoftwareInvariant,
            _ => Class::ConfigurationBoot,
        }
    }

    fn from_word(word: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|failure| *failure as u32 == word)
    }
}

/// Record a failed boot check for the next boot, whose note the console's
/// driver kept off the block. Never waits: while the other hart records its
/// own failure, this one's is skipped.
pub(crate) fn record_failure(claim: &Claim<'_>, failure: Failure) {
    let hart = TrapRegisters::read().hart as u32;
    if let Ok(record) = Record::encode(
        failure.class(),
        Domain::Hil,
        FAILURE,
        hart,
        0,
        &(failure as u32),
    ) {
        let _committed = claim.commit(&SLOT, record);
    }
}

/// Print the previous record early at boot; acknowledge only after its note
/// was written whole. A record of another shape stays unprinted.
pub(crate) fn report_previous() {
    if let Some(mut slot) = retained_fault::peek(&SLOT) {
        let _reported = slot.report(|view| {
            if view.reason != FAILURE {
                return Err(());
            }
            let word = view.decode::<u32>(Domain::Hil).map_err(|_| ())?;
            let failure = Failure::from_word(word).ok_or(())?;
            crate::usb::early(&Note::fatal_fail(view.hart, failure.name()))
                .is_whole()
                .then_some(())
                .ok_or(())
        });
    }
}
