//! Bounded AP beacon storage and its TSF-driven publication.
//!
//! The beacon is a template the access point prepares for publication: the
//! management sequence, the TIM and the DTIM count of its TSF's value (its
//! own time since its TSF restarted). The Timestamp field goes zero: the
//! transmitter writes the access point's TSF there as the beacon goes out
//! (`BeaconTimestamp::Transmitter`). Its schedule is an absolute TBTT cursor
//! that a late publication does not move.
//!
//! A move of the BSS to another channel is announced in the template: the
//! Channel Switch Announcement (and, for a 40 MHz channel, the Secondary
//! Channel Offset) follows the TIM, and each publication writes the beacons
//! left until the switch. After the beacon that counts one the switch is
//! due at the next TBTT; the access point's owner moves the port and the
//! template is written again for the new channel ([`ApBeacon::rewrite`]).

use oer_protocols_ieee80211_wire::beacon::{
    BeaconTemplateError, dtim, insert_after_tim, remove_elements, write_channel_switch_count,
};
use oer_protocols_ieee80211_wire::channel_switch::{
    CHANNEL_SWITCH_ELEMENT_ID, ChannelSwitch, EXTENDED_CHANNEL_SWITCH_ELEMENT_ID,
    SECONDARY_CHANNEL_OFFSET_ELEMENT_ID,
};
use oer_protocols_ieee80211_wire::frame::write_sequence_control;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use oer_protocols_ieee80211_wire::tsf::time_units_micros;

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_wire::{
    ap::profile::Advertisement,
    beacon::{
        AP_BEACON_CAPACITY, ApBeaconBuildError, ApBeaconProtectionError, TimPartialVirtualBitmap,
        stamp, update_bss_protection, write_ht_beacon, write_tim_partial_virtual_bitmap,
    },
    channel::Channel,
    protection::ApBssProtection,
    ssid::Ssid,
};

use crate::{security_policy::ApSecurityPolicy, tbtt::next_tbtt};

mod publication;
pub use publication::{ApBeaconPublication, ApBeaconTxContext, ApBeaconTxOutcome, ApDtimRelease};

/// An initialized template length outside its fixed storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApBeaconLengthError {
    pub length: usize,
    pub capacity: usize,
}

/// A refused [`ApBeacon`] construction: why, and the storage it was given.
#[must_use = "a refused beacon returns its storage"]
#[derive(Debug)]
pub struct ApBeaconRefused<'storage> {
    /// Why it was refused.
    pub error: ApBeaconBuildError,
    /// The storage it was given; its bytes are not a template.
    pub storage: &'storage mut [u8; AP_BEACON_CAPACITY],
}

pub struct ApBeacon<'storage> {
    storage: &'storage mut [u8; AP_BEACON_CAPACITY],
    len: usize,
    interval: Duration,
    /// Absolute TBTT following the most recently published beacon.
    ///
    /// This is a schedule cursor, not the actual publication timestamp.
    /// Complete vendor `wdev.o::wDev_Get_Next_TBTT` advances persistent
    /// `BcnSendTick` by `BcnInterval` and stores every catch-up step before
    /// returning the delay. Keeping the cursor independent of executor jitter
    /// prevents one late publication from moving every later TBTT.
    next_publication: Option<Instant>,
    /// The announced move of the BSS: its count is the beacons left until
    /// the switch, zero once it is due.
    channel_switch: Option<ChannelSwitch>,
}

/// Why a channel switch cannot be announced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApChannelSwitchError {
    /// An announcement counts at least one beacon: the access point's
    /// stations learn of the move before it happens.
    ZeroCount,
    /// The template has no TIM to place the announcement after.
    MalformedTemplate,
    /// The template's storage cannot hold the announcement.
    Full,
}

const fn template_error(error: BeaconTemplateError) -> ApChannelSwitchError {
    match error {
        BeaconTemplateError::Malformed => ApChannelSwitchError::MalformedTemplate,
        BeaconTemplateError::Full => ApChannelSwitchError::Full,
    }
}

impl<'storage> ApBeacon<'storage> {
    /// The beacon's body, which a probe response repeats.
    pub fn advertisement(&self) -> Result<&[u8], ApBeaconLengthError> {
        self.storage.get(..self.len).ok_or(ApBeaconLengthError {
            length: self.len,
            capacity: AP_BEACON_CAPACITY,
        })
    }

    /// Replace the template's ERP and HT Operation protection fields.
    pub fn set_bss_protection(
        &mut self,
        protection: ApBssProtection,
    ) -> Result<(), ApBeaconProtectionError> {
        update_bss_protection(
            self.storage
                .get_mut(..self.len)
                .ok_or(ApBeaconProtectionError::MalformedElement)?,
            protection,
        )
    }

    /// # Errors
    ///
    /// [`ApBeaconRefused`]: the template does not fit or an input is out of
    /// range; the storage comes back (its bytes are not a template).
    #[expect(
        clippy::too_many_arguments,
        reason = "each beacon field is an independent 802.11 input the caller owns"
    )]
    pub fn new(
        storage: &'storage mut [u8; AP_BEACON_CAPACITY],
        advertisement: &Advertisement,
        access_point: [u8; 6],
        ssid: &Ssid,
        channel: Channel,
        beacon_interval_tu: u16,
        dtim_period: u8,
        management_sequence: SequenceNumber,
        security: ApSecurityPolicy,
    ) -> Result<Self, ApBeaconRefused<'storage>> {
        let written = write_ht_beacon(
            advertisement,
            storage,
            access_point,
            ssid,
            channel,
            beacon_interval_tu,
            dtim_period,
            management_sequence,
            security.advertisement(),
            ApBssProtection::default(),
        );
        let len = match written {
            Ok(len) => len,
            Err(error) => return Err(ApBeaconRefused { error, storage }),
        };
        Ok(Self {
            storage,
            len,
            interval: Duration::from_micros(time_units_micros(beacon_interval_tu)),
            next_publication: None,
            channel_switch: None,
        })
    }

    /// Write the template again, for the BSS on `channel`: the move an
    /// announcement named is done and the announcement goes. The TBTT
    /// schedule stays; the protection fields restart from none.
    #[expect(
        clippy::too_many_arguments,
        reason = "each beacon field is an independent 802.11 input the caller owns"
    )]
    pub fn rewrite(
        &mut self,
        advertisement: &Advertisement,
        access_point: [u8; 6],
        ssid: &Ssid,
        channel: Channel,
        beacon_interval_tu: u16,
        dtim_period: u8,
        management_sequence: SequenceNumber,
        security: ApSecurityPolicy,
    ) -> Result<(), ApBeaconBuildError> {
        self.len = write_ht_beacon(
            advertisement,
            self.storage,
            access_point,
            ssid,
            channel,
            beacon_interval_tu,
            dtim_period,
            management_sequence,
            security.advertisement(),
            ApBssProtection::default(),
        )?;
        self.interval = Duration::from_micros(time_units_micros(beacon_interval_tu));
        self.channel_switch = None;
        Ok(())
    }

    /// Announce `switch` in every beacon from the next one on, in place of
    /// an earlier announcement: its count is the beacons left, the first of
    /// them included.
    pub fn announce_channel_switch(
        &mut self,
        switch: ChannelSwitch,
    ) -> Result<(), ApChannelSwitchError> {
        if switch.count == 0 {
            return Err(ApChannelSwitchError::ZeroCount);
        }
        let mut elements = [0_u8; 8];
        let added = switch
            .encode_elements(&mut elements)
            .ok_or(ApChannelSwitchError::Full)?;
        self.remove_channel_switch_elements()?;
        let elements = elements.get(..added).ok_or(ApChannelSwitchError::Full)?;
        let end = insert_after_tim(self.storage, self.len, elements).map_err(template_error)?;
        self.len = end;
        self.channel_switch = Some(switch);
        Ok(())
    }

    /// The announced move, its count the beacons left until it.
    pub const fn channel_switch(&self) -> Option<ChannelSwitch> {
        self.channel_switch
    }

    /// The announced move, once the beacon that counted one went out: the
    /// switch happens at the next TBTT, before the next beacon.
    pub fn channel_switch_due(&self) -> Option<ChannelSwitch> {
        self.channel_switch.filter(|switch| switch.count == 0)
    }

    /// Take the announcement's elements out of the template.
    fn remove_channel_switch_elements(&mut self) -> Result<(), ApChannelSwitchError> {
        self.len = remove_elements(
            self.storage,
            self.len,
            &[
                CHANNEL_SWITCH_ELEMENT_ID,
                EXTENDED_CHANNEL_SWITCH_ELEMENT_ID,
                SECONDARY_CHANNEL_OFFSET_ELEMENT_ID,
            ],
        )
        .map_err(template_error)?;
        Ok(())
    }

    /// Write the beacons left into the announcement's count.
    fn write_channel_switch_count(&mut self, count: u8) -> Option<()> {
        write_channel_switch_count(self.storage, self.len, count)
    }

    /// A beacon over a template already written into `storage`.
    pub const fn from_initialized(
        storage: &'storage mut [u8; AP_BEACON_CAPACITY],
        len: usize,
        beacon_interval_tu: u16,
    ) -> Result<Self, ApBeaconLengthError> {
        if len > AP_BEACON_CAPACITY {
            return Err(ApBeaconLengthError {
                length: len,
                capacity: AP_BEACON_CAPACITY,
            });
        }
        Ok(Self {
            storage,
            len,
            interval: Duration::from_micros(time_units_micros(beacon_interval_tu)),
            next_publication: None,
            channel_switch: None,
        })
    }

    /// Prepare one frame at the access point's TSF value `now` (its DTIM
    /// count; the Timestamp field zero, which the transmitter writes) and
    /// advance its TBTT schedule.
    ///
    /// The returned publication owns the exact group queue prefix advertised
    /// by this frame. Retain its context until the TX terminal; only a
    /// delivered beacon grants that prefix's release. Later queue arrivals
    /// and later TIM phases cannot enlarge it.
    ///
    ///
    /// `None` also while an announced switch is due: no beacon goes out on
    /// the old channel after the one that counted one.
    pub fn prepare(
        &mut self,
        now: Instant,
        management_sequence: SequenceNumber,
        buffered_group_frames: u16,
        unicast_tim_bitmap: TimPartialVirtualBitmap<'_>,
    ) -> Option<ApBeaconPublication<'_>> {
        if self.channel_switch_due().is_some() {
            return None;
        }
        stamp(
            self.storage.get_mut(..self.len)?,
            now.as_micros(),
            buffered_group_frames != 0,
        )?;
        write_sequence_control(self.storage, management_sequence.sequence_control())?;
        self.len = write_tim_partial_virtual_bitmap(self.storage, self.len, unicast_tim_bitmap)?;
        if let Some(count) = self.channel_switch.map(|switch| switch.count) {
            self.write_channel_switch_count(count)?;
            if let Some(switch) = &mut self.channel_switch {
                switch.count = switch.count.checked_sub(1)?;
            }
        }
        let schedule_base = self.next_publication.unwrap_or(now);
        self.next_publication = Some(next_tbtt(schedule_base, self.interval, now)?);
        let frame = self.storage.get_mut(..self.len)?;
        let (_, dtim_count, _) = dtim(frame)?;
        Some(ApBeaconPublication::new(
            frame,
            if dtim_count == 0 {
                buffered_group_frames
            } else {
                0
            },
        ))
    }

    /// The next TBTT, once the first beacon has established the schedule.
    /// Forget the TBTT schedule: the next publication starts a new one, as
    /// a BSS that starts again from a restarted TSF does.
    pub fn restart_schedule(&mut self) {
        self.next_publication = None;
    }

    /// Start the TBTT schedule at `first`: the first beacon is due then, and
    /// every later TBTT lies a whole number of intervals after it. A BSS
    /// whose TSF restarted from zero starts it at zero, so its TBTTs are the
    /// TSF's multiples of the interval (IEEE Std 802.11-2020, 11.1.3.2).
    pub fn start_schedule(&mut self, first: Instant) {
        self.next_publication = Some(first);
    }

    /// The beacon interval.
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    pub const fn next_publication(&self) -> Option<Instant> {
        if self.interval.as_micros() == 0 {
            return None;
        }
        self.next_publication
    }

    /// Whether the current beacon interval has elapsed since publication.
    ///
    /// `next_tbtt` deliberately skips an already missed TBTT, matching
    /// the recovered vendor timer calculation. An executor which was occupied
    /// by an uninterruptible TX exchange must test this edge first, otherwise
    /// repeated completions just after TBTT can postpone every beacon by one
    /// more interval.
    pub fn publication_due(&self, now: Instant) -> bool {
        match self.next_publication {
            Some(next) => now >= next,
            // A newly started AP has no TBTT cursor until its first beacon is
            // handed to hardware.  Treat that uninitialized epoch as due so
            // every composition (standalone or paired VIF) establishes the
            // same absolute schedule before it attempts to wait on it.
            None => true,
        }
    }

    /// Return complete beacon intervals skipped after the next expected
    /// publication and the exact lateness beyond that publication. The first
    /// publication establishes the epoch and therefore has no preceding
    /// deadline to miss.
    ///
    /// These are deliberately separate facts: a publication can be almost a
    /// full interval late without crossing the following interval boundary.
    /// Qualification must bound `lateness` independently rather than
    /// relabeling every nonzero scheduler delay as a skipped interval.
    pub fn publication_lateness(&self, now: Instant) -> (u64, Duration) {
        let Some(next) = self.next_publication else {
            return (0, Duration::ZERO);
        };
        let interval = self.interval.as_micros();
        let lateness = now.saturating_duration_since(next);
        let Some(skipped) = lateness.as_micros().checked_div(interval) else {
            return (0, Duration::ZERO);
        };
        (skipped, lateness)
    }

    pub fn into_storage(self) -> &'storage mut [u8; AP_BEACON_CAPACITY] {
        self.storage
    }
}

#[cfg(test)]
mod tests;
