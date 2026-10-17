//! The values of the lower-MAC port's optional features: A-MPDU
//! transmission, beacon timing (each interface's TSF and TBTT schedule)
//! and monitor reception, with their parametric limits. Why a backend
//! lacks a feature is recorded in its documentation, not in code.

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};

use crate::{
    capabilities::PhyFormatSet,
    control::{VifRole, VifRoleSet},
};

/// The limits of A-MPDU attempts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AmpduCapabilities {
    /// Subframes of one A-MPDU attempt.
    pub max_subframes: u16,
    /// The PPDU formats an A-MPDU attempt is sent in; an attempt at a rate
    /// of another format has no checked aggregate rate.
    pub formats: PhyFormatSet,
    /// The longest A-MPDU in octets, delimiters and padding included, that
    /// the backend sends at every rate of `formats`. The recipient's own
    /// Maximum A-MPDU Length stays the caller's to respect, as does the
    /// TXOP limit of the attempt's access category: the caller keeps the
    /// aggregate and its BlockAck within it, and a backend refuses no
    /// aggregate by it.
    pub max_length: u32,
}

/// A value of one interface's TSF. Two interfaces count different TSFs (a
/// station follows its access point's, an access point keeps its own), so
/// arithmetic between values of different interfaces is an error.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VifTsf {
    /// The interface whose TSF this value belongs to.
    pub vif: VifRole,
    /// The reading in that interface's TSF epoch.
    pub at: TsfInstant,
}

/// Two TSF values of different interfaces were combined.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TsfVifMismatch {
    /// The interface of the left-hand operand.
    pub left: VifRole,
    /// The interface of the right-hand operand.
    pub right: VifRole,
}

/// Failure of arithmetic between interface-scoped TSF values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TsfArithmeticError {
    /// The operands refer to different interfaces.
    VifMismatch(TsfVifMismatch),
    /// The purported earlier value lies after the later one.
    ReversedTime,
}

impl VifTsf {
    /// Associate a TSF reading with its interface.
    pub const fn new(vif: VifRole, at: TsfInstant) -> Self {
        Self { vif, at }
    }

    /// The same interface's TSF `duration` later; `None` beyond its epoch.
    pub const fn checked_add(self, duration: TsfDuration) -> Option<Self> {
        match self.at.checked_add(duration) {
            Some(at) => Some(Self { vif: self.vif, at }),
            None => None,
        }
    }

    /// Same-interface elapsed radio time. Interface mismatch and reversal
    /// are explicit errors; neither produces a fabricated duration.
    pub const fn checked_duration_since(
        self,
        earlier: Self,
    ) -> Result<TsfDuration, TsfArithmeticError> {
        if self.vif as u8 != earlier.vif as u8 {
            return Err(TsfArithmeticError::VifMismatch(TsfVifMismatch {
                left: self.vif,
                right: earlier.vif,
            }));
        }
        match self.at.checked_duration_since(earlier.at) {
            Some(elapsed) => Ok(elapsed),
            None => Err(TsfArithmeticError::ReversedTime),
        }
    }
}

/// One target beacon transmission time of an interface's schedule.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TbttEvent {
    /// The TBTT the event announces, in its interface's TSF.
    pub tbtt: VifTsf,
}

/// The largest rate difference between an interface's TSF and the port's
/// radio clock, in parts per million.
///
/// IEEE 802.11-2020 11.1.3 bounds every station's timer at ±0.01 %
/// (100 ppm). A station's TSF follows the access point's timer while the
/// radio clock runs on the station's own crystal, and the two may err in
/// opposite directions: the bound is the sum of both sides. A narrower
/// bound needs a cited tolerance of the station's crystal.
pub const TSF_DRIFT_PPM: u32 = 200;

/// One paired reading of an interface's TSF and the port's radio clock,
/// taken back to back by the backend, in the generation of their relation.
///
/// The relation breaks, and the generation changes, when the TSF jumps (a
/// new BSS, a reassociation, a channel change, a TSF set beyond the
/// backend's tolerance, an access point's TSF restart) and with every break
/// of the radio clock itself (its stamp's generation).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfSample {
    /// The interface's TSF reading beside `local`.
    pub tsf: VifTsf,
    /// The port's radio clock beside it.
    pub local: crate::Ieee80211Stamp,
    /// How far apart the two readings may lie, on the port's radio clock.
    pub uncertainty: crate::Ieee80211Duration,
    /// The TSF relation the sample belongs to.
    pub generation: TsfGeneration,
}

/// The generation of a TSF relation: the epoch its owner took when it was
/// created, from a source shared by every owner of one radio start, and the
/// jumps since. Two owners never share an epoch, so a generation names one
/// stretch of one relation. Only the relation's [`TsfRelation`] makes one.
pub type TsfGeneration = oer_radio_port::Generation<TsfInstant, crate::Ieee80211Instant>;

/// How an interface's TSF advances against the port's radio clock: at its
/// rate within [`TSF_DRIFT_PPM`] (the access point's timer and the
/// station's crystal).
pub const TSF_RELATION: oer_radio_port::ClockRelation<TsfInstant, crate::Ieee80211Instant> =
    oer_radio_port::ClockRelation::affine(TSF_DRIFT_PPM);

/// The relation of one interface's TSF to the radio clock across TSF sets:
/// its generation and its last sample, the value the TSF was last set to.
///
/// The backend that owns an interface's TSF writes keeps one and records
/// every write. A set within what the relation predicts since its last
/// sample keeps the generation: [`TSF_DRIFT_PPM`] of the time since then,
/// rounded up, plus the sample's uncertainty. A beacon follow after missed
/// beacons corrects more drift and stays within it; a larger set, and the
/// first one, is a jump and starts a new generation. The TSF is not modular
/// here: a set to a value across 2^64, or one after the counter passed 2^64
/// since the last sample (a reading below it), is a jump, so the instants of
/// one generation always compare in order.
#[derive(Debug, Eq, PartialEq)]
pub struct TsfRelation {
    generations: oer_radio_port::GenerationSource<TsfInstant, crate::Ieee80211Instant>,
    last_sample: LastSample,
    sample_uncertainty: TsfDuration,
}

/// The value the TSF was last set to in the current generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LastSample {
    /// No set since the relation began or broke: the next set is a jump.
    Unanchored,
    /// The last set's value.
    Set(TsfInstant),
}

/// What a TSF set did to its relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfSetKind {
    /// Within the drift since the last sample: the generation stays.
    Drift,
    /// Beyond it, or the first set: a new generation.
    Jump,
}

/// A TSF set could not establish a representable timing relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfTimingError {
    /// An elapsed span could not order its current and earlier samples.
    ReversedTime,
    /// Drift plus sample uncertainty exceeds the TSF duration range.
    ToleranceOverflow,
}

impl TsfRelation {
    /// A relation without a sample in `epoch`, a number its owner took from
    /// the shared source, whose samples are each uncertain by
    /// `sample_uncertainty` (the backend's TSF counter resolution).
    pub const fn new(epoch: u32, sample_uncertainty: TsfDuration) -> Self {
        Self {
            generations: oer_radio_port::GenerationSource::new(epoch),
            last_sample: LastSample::Unanchored,
            sample_uncertainty,
        }
    }

    /// The current generation.
    pub const fn generation(&self) -> TsfGeneration {
        self.generations.current()
    }

    /// Start a new generation without a sample: the TSF jumped (a new
    /// association, a channel change, a timer restart).
    pub fn break_relation(&mut self) {
        self.generations.jump();
        self.last_sample = LastSample::Unanchored;
    }

    /// How far a set `elapsed` after the last sample may move the TSF
    /// without breaking the relation.
    pub fn tolerance(&self, elapsed: TsfDuration) -> Result<TsfDuration, TsfTimingError> {
        // The product of a u64 span and the fixed ppm coefficient fits u128.
        let drift = u128::from(elapsed.as_micros())
            .checked_mul(u128::from(TSF_DRIFT_PPM))
            .ok_or(TsfTimingError::ToleranceOverflow)?
            .div_ceil(1_000_000);
        let drift = u64::try_from(drift).map_err(|_| TsfTimingError::ToleranceOverflow)?;
        TsfDuration::from_micros(drift)
            .checked_add(self.sample_uncertainty)
            .ok_or(TsfTimingError::ToleranceOverflow)
    }

    /// Record a set of the TSF from `current`, the value read just before
    /// it, to `value`.
    pub fn set(
        &mut self,
        current: TsfInstant,
        value: TsfInstant,
    ) -> Result<TsfSetKind, TsfTimingError> {
        let kind = match self.last_sample {
            // The counter only advances from the last set: a reading below
            // it passed 2^64.
            LastSample::Set(last) if current < last => TsfSetKind::Jump,
            LastSample::Set(last) => {
                let elapsed = current
                    .checked_duration_since(last)
                    .ok_or(TsfTimingError::ReversedTime)?;
                let moved = value.as_micros().abs_diff(current.as_micros());
                if moved <= self.tolerance(elapsed)?.as_micros() {
                    TsfSetKind::Drift
                } else {
                    TsfSetKind::Jump
                }
            }
            LastSample::Unanchored => TsfSetKind::Jump,
        };
        if kind == TsfSetKind::Jump {
            self.break_relation();
        }
        self.last_sample = LastSample::Set(value);
        Ok(kind)
    }
}

/// Why a TSF projection has no value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfProjectionError {
    /// The radio stamp belongs to another generation of the radio clock.
    StaleStamp,
    /// The sample belongs to another generation of the TSF relation.
    StaleSample,
    /// The projected value lies outside the representable range.
    OutOfRange,
}

/// The sample is one reading of the relation between its interface's TSF
/// (the source) and the port's radio clock (the target); its projections
/// are those of [`TSF_RELATION`], with the interface and the radio clock's
/// own generation checked on top.
impl TsfSample {
    /// The relation sample of this reading, without its interface.
    fn relation(&self) -> oer_radio_port::RelationSample<TsfInstant, crate::Ieee80211Instant> {
        oer_radio_port::RelationSample {
            source: self.tsf.at,
            target: self.local.at,
            uncertainty: self.uncertainty,
            generation: self.generation,
        }
    }

    /// The TSF of this sample's interface when the port's radio clock read
    /// `local`, with the sample's uncertainty plus the drift over the
    /// distance from the sample; `current` is the relation's generation now.
    pub fn tsf_at(
        &self,
        local: crate::Ieee80211Stamp,
        current: TsfGeneration,
    ) -> Result<oer_radio_port::Projected<VifTsf, TsfDuration>, TsfProjectionError> {
        self.check(current)?;
        if local.generation != self.local.generation {
            return Err(TsfProjectionError::StaleStamp);
        }
        let projected = TSF_RELATION.project_back(local.at, &self.relation())?;
        Ok(oer_radio_port::Projected {
            at: VifTsf::new(self.tsf.vif, projected.at.at),
            uncertainty: projected.uncertainty,
        })
    }

    /// The port's radio clock when this sample's interface's TSF reads
    /// `tsf`, in the sample's radio-clock generation; the inverse of
    /// [`Self::tsf_at`]. An error for a TSF of another interface.
    pub fn local_at(
        &self,
        tsf: VifTsf,
        current: TsfGeneration,
    ) -> Result<
        Result<
            oer_radio_port::Projected<crate::Ieee80211Stamp, crate::Ieee80211Duration>,
            TsfProjectionError,
        >,
        TsfVifMismatch,
    > {
        if tsf.vif as u8 != self.tsf.vif as u8 {
            return Err(TsfVifMismatch {
                left: tsf.vif,
                right: self.tsf.vif,
            });
        }
        Ok((|| {
            self.check(current)?;
            let projected = TSF_RELATION.project(tsf.at, current, &self.relation())?;
            Ok(oer_radio_port::Projected {
                at: crate::Ieee80211Stamp {
                    at: projected.at,
                    generation: self.local.generation,
                },
                uncertainty: projected.uncertainty,
            })
        })())
    }

    /// Whether the sample belongs to the relation's `current` generation.
    fn check(&self, current: TsfGeneration) -> Result<(), TsfProjectionError> {
        if self.generation == current {
            Ok(())
        } else {
            Err(TsfProjectionError::StaleSample)
        }
    }
}

impl From<oer_radio_port::ProjectionError> for TsfProjectionError {
    fn from(error: oer_radio_port::ProjectionError) -> Self {
        match error {
            oer_radio_port::ProjectionError::StaleSample => Self::StaleSample,
            oer_radio_port::ProjectionError::OutOfRange => Self::OutOfRange,
        }
    }
}

/// Which roles the beacon-timing operations serve.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BeaconTimingCapabilities {
    /// Roles whose TSF `LowerMacBeaconTiming::tsf` reads.
    pub tsf_read: VifRoleSet,
    /// Roles whose TSF `LowerMacBeaconTiming::set_tsf` sets to any value.
    pub tsf_set: VifRoleSet,
    /// Roles whose TSF `LowerMacBeaconTiming::set_tsf` restarts from zero.
    pub tsf_restart: VifRoleSet,
    /// Roles whose TBTT schedule `LowerMacBeaconTiming::set_tbtt`
    /// programs.
    pub tbtt: VifRoleSet,
}

/// The limits of monitor reception.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MonitorCapabilities {
    /// Whether monitor reception runs while an interface has a receive
    /// filter other than `ReceiveFilter::NONE`.
    pub with_receiving_interfaces: bool,
}
