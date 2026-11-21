//! Typed HE trigger/OFDMA control and best-effort hardware diagnostics.

use oer_portable_mmio::{Bits, Index};

use crate::radio::ieee80211::registers::{
    MacPlacement, ORDINARY_QUEUE_COUNT, WifiRadioRegisters, byte, element, halfword,
};

/// One IEEE 802.11 traffic identifier accepted by the recovered HE BSR block.
///
/// Keeping this as a bounded newtype means array and MMIO indexing cannot
/// receive an arbitrary `u8`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTid(Bits<{ oer_chips_esp32s31_pac::wifi_mac::queue_control::TID_WIDTH }>);

impl MacHeTid {
    pub const COUNT: usize = oer_chips_esp32s31_pac::wifi_mac::hardware_bsr::COUNT;

    pub const fn new(value: u8) -> Option<Self> {
        if value >= Self::COUNT as u8 {
            return None;
        }
        match Bits::new(value as u32) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    pub const fn value(self) -> u8 {
        // Below `COUNT`: the low byte is the whole value.
        self.0.get().to_le_bytes()[0]
    }

    /// The TID as the queue control's field.
    pub const fn field(
        self,
    ) -> Bits<{ oer_chips_esp32s31_pac::wifi_mac::queue_control::TID_WIDTH }> {
        self.0
    }

    const fn mask(self) -> u8 {
        1 << self.value()
    }
}

/// One MPDU length of a Trigger-based queue: non-zero, within the
/// fourteen-bit length of its link entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeMpduLength(
    Bits<{ oer_chips_esp32s31_pac::wifi_mac::he_scratch::MPDU_LENGTH_WIDTH }>,
);

impl MacHeMpduLength {
    /// A length the link entry holds, or none for zero or a wider one.
    pub const fn new(length: u16) -> Option<Self> {
        if length == 0 {
            return None;
        }
        match Bits::new(length as u32) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// The length as its link entry's field.
    pub const fn field(
        self,
    ) -> Bits<{ oer_chips_esp32s31_pac::wifi_mac::he_scratch::MPDU_LENGTH_WIDTH }> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum MacHeTbTidLimit {
    One = 1,
    Two = 2,
    #[default]
    Three = 3,
    Four = 4,
}

impl MacHeTbTidLimit {
    /// Exact eligible-TID bitmap returned by the pinned blob.
    const fn bitmap(self) -> u8 {
        match self {
            Self::One => 0x01,
            Self::Two => 0x81,
            Self::Three => 0xa1,
            Self::Four => 0xa3,
        }
    }

    pub const fn contains(self, tid: MacHeTid) -> bool {
        self.bitmap() & tid.mask() != 0
    }
}

/// Exclusive contiguous reservation in the 120-entry HE MPDU-length table.
///
/// The open owner permits one live aggregate per hardware queue, so the
/// vendor allocator's disjoint queue ranges can be used deterministically
/// without a global bitmap or dynamic allocation.
/// One entry of the 120-entry HE MPDU-length table: its link index.
pub type MacHeTbLink = Index<{ oer_chips_esp32s31_pac::wifi_mac::he_scratch::COUNT }>;

/// The entry an MPDU-length entry links to: another entry, or the end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacHeTbNextLink {
    Link(MacHeTbLink),
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTbLinkReservation {
    queue: Index<{ ORDINARY_QUEUE_COUNT as usize }>,
    first: MacHeTbLink,
    count: u8,
}

impl MacHeTbLinkReservation {
    /// Reserve the beginning of the exact range selected by the blob.
    ///
    /// SOURCE: complete
    /// `libpp[hal_he_common.o]::hal_he_get_mplen_addr_start,libpp[hal_he_common.o]::hal_he_get_mplen_addr_end` and the
    /// corresponding ROM leaves. The single-TID layout differs from all
    /// other policies.
    pub const fn for_queue(
        policy: MacHeTbTidLimit,
        queue: Index<{ ORDINARY_QUEUE_COUNT as usize }>,
        count: u8,
    ) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let single = matches!(policy, MacHeTbTidLimit::One);
        // Each range's first entry and its capacity, by the blob's tables.
        let (first, capacity): (MacHeTbLink, u8) = match (single, queue.get()) {
            (true, 0) => (Index::constant::<64>(), 18),
            (true, 1) => (Index::constant::<82>(), 18),
            (true, 2) => (Index::constant::<0>(), 64),
            (true, _) => (Index::constant::<100>(), 18),
            (false, 0) => (Index::constant::<32>(), 32),
            (false, 1) => (Index::constant::<64>(), 32),
            (false, 2) => (Index::constant::<0>(), 32),
            (false, _) => (Index::constant::<96>(), 24),
        };
        if count > capacity {
            return None;
        }
        Some(Self {
            queue,
            first,
            count,
        })
    }

    pub const fn queue(self) -> Index<{ ORDINARY_QUEUE_COUNT as usize }> {
        self.queue
    }

    pub const fn first(self) -> u8 {
        // Below 120: the low byte is the whole index.
        self.first.get().to_le_bytes()[0]
    }

    pub const fn count(self) -> u8 {
        self.count
    }

    /// The entry of `position`, inside the reservation.
    const fn index(self, position: u8) -> Option<MacHeTbLink> {
        if position >= self.count {
            return None;
        }
        match Index::new(self.first.get().wrapping_add(position as usize)) {
            Ok(link) => Some(link),
            Err(_) => None,
        }
    }

    /// The entry `position` links to: the next one, or the end for the last.
    const fn next(self, position: u8) -> Option<MacHeTbNextLink> {
        if position >= self.count {
            None
        } else if position as usize + 1 == self.count as usize {
            Some(MacHeTbNextLink::End)
        } else {
            match self.index(position + 1) {
                Some(link) => Some(MacHeTbNextLink::Link(link)),
                None => None,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeTbProgramError {
    TidNotEligible,
    LengthCountMismatch,
    QueuedBytesTooLarge,
}

/// Coherent-enough readback of one just-published Trigger-eligible TX queue.
///
/// The caller must still own the corresponding live reservation. Reading
/// after completion/cleanup can legitimately observe hardware-updated state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTriggerTxQueueSnapshot {
    pub logical_queue: u8,
    pub tid: u8,
    pub trigger_based_enabled: bool,
    pub mu_edca_timer_select: u8,
    pub mu_edca_timer_enabled: bool,
    pub first_link: u8,
    pub first_mpdu_length: u16,
    pub first_next_link: u8,
    pub tail_link: u8,
    /// Low-twenty-bit value supplied to the software-BSR write.
    pub programmed_msdu_bytes: u32,
    /// Low-twenty-bit sum of original frame-state `msdu_len` values.
    ///
    /// This second sample is taken after the validity publication edge.
    pub queued_msdu_bytes: u32,
    pub queue_valid: bool,
}

/// The CONF1 element of logical trigger queue `queue`: the four ordinary
/// queues' words are the last four of eight, in reverse logical order.
const fn trigger_queue_element(queue: Index<{ ORDINARY_QUEUE_COUNT as usize }>) -> Index<8> {
    match queue.get() {
        0 => Index::constant::<7>(),
        1 => Index::constant::<6>(),
        2 => Index::constant::<5>(),
        _ => Index::constant::<4>(),
    }
}

/// The physical bank of a reservation's ordinary queue: the banks are in
/// reverse logical order.
const fn reservation_bank(reservation: MacHeTbLinkReservation) -> Index<4> {
    match reservation.queue.get() {
        0 => Index::constant::<3>(),
        1 => Index::constant::<2>(),
        2 => Index::constant::<1>(),
        _ => Index::constant::<0>(),
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Program the complete write-side HE Trigger-eligible queue transition.
    ///
    /// SOURCE: instruction-equivalent complete
    /// `libpp[hal_mac_tx.o]::mac_tx_set_tb,
    /// libpp[hal_mac_tx.o]::mac_tx_set_mplen` and rev0 ROM bodies `rom[]::mac_tx_set_tb` at
    /// `0x2f834f82` and `rom[]::mac_tx_set_mplen` at `0x2f836b1a`.
    ///
    /// `reservation` represents exclusive software ownership of this
    /// hardware queue's disjoint MPDU-link range. The final validity RMW is
    /// the publication edge; every link, timer, queue-control, tail and BSR
    /// write is device-visible before it.
    pub fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[MacHeMpduLength],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        if !policy.contains(tid) {
            return Err(MacHeTbProgramError::TidNotEligible);
        }
        if mpdu_lengths.len() != usize::from(reservation.count) {
            return Err(MacHeTbProgramError::LengthCountMismatch);
        }
        let queued: Bits<{ oer_chips_esp32s31_pac::wifi_mac::software_bsr::VALUE_WIDTH }> =
            Bits::new(queued_msdu_bytes).map_err(|_| MacHeTbProgramError::QueuedBytesTooLarge)?;

        let queue = reservation.queue;
        let physical_queue = trigger_queue_element(queue);

        // mac_tx_set_tb enables the selected MU-EDCA timer before allocating
        // the first MPDU-length entry.
        self.mac
            .wifi_mac_he_mu_edca_timer
            .modify_timer(element(queue.get()), |w| {
                w.set_enable(true);
            });

        self.write_he_mpdu_length_link(reservation, 0, mpdu_lengths[0]);

        // The setter preserves exactly bit two of the old CONF1 image and
        // constructs every other named field from the current frame/queue:
        // one store of every field, the unnamed ones zero.
        let suffix = &mut self.mac.wifi_mac_he_init_suffix;
        let qos_null_to_translated_bss = suffix
            .read_queue_control(physical_queue)
            .qos_null_to_translated_bss();
        suffix.modify_queue_control(physical_queue, |w| {
            w.set_low_cleared_by_tb_setup(Bits::constant::<0>())
                .set_qos_null_to_translated_bss(qos_null_to_translated_bss)
                .set_trigger_based_enable(true)
                .set_mu_edca_timer_select(Bits::from(queue))
                .set_mpdu_length_link_address(Bits::from(reservation.first))
                .set_tid(tid.field())
                .set_minimum_tx_power(Bits::constant::<0>())
                .set_high_cleared_by_tb_setup(Bits::constant::<0>());
        });

        for (position, length) in mpdu_lengths.iter().enumerate().skip(1) {
            let position =
                u8::try_from(position).expect("a reservation holds at most 120 positions");
            self.write_he_mpdu_length_link(reservation, position, *length);
        }

        let status = &mut self.mac.wifi_mac_he_buffer_status;
        status.modify_software_bsr(element(queue.get()), |w| {
            w.set_value(queued);
        });
        // Do not add a fence or diagnostic read here. The complete blob and
        // ROM sequence is exactly SW(BSR), LW(CONTROL), SW(CONTROL). BSR
        // hardware update is live, so lengthening the interval before the
        // validity RMW changes observable behavior.
        status.modify_he_buffer_status_control(|w| {
            match queue.get() {
                0 => w.set_valid_0(true),
                1 => w.set_valid_1(true),
                2 => w.set_valid_2(true),
                _ => w.set_valid_3(true),
            };
        });
        let mut snapshot = self.he_trigger_based_queue_snapshot(reservation);
        snapshot.programmed_msdu_bytes = queued_msdu_bytes;
        Ok(snapshot)
    }

    /// Link one MPDU-length entry of `reservation` and, for its terminal
    /// entry, publish the queue's tail link.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_set_mplen`.
    fn write_he_mpdu_length_link(
        &mut self,
        reservation: MacHeTbLinkReservation,
        position: u8,
        length: MacHeMpduLength,
    ) {
        let index = reservation
            .index(position)
            .expect("each linked position lies inside the reservation");
        let next = reservation
            .next(position)
            .expect("each linked position lies inside the reservation");
        self.mac
            .wifi_mac_he_init_suffix
            .modify_he_scratch(index, |w| {
                w.set_mpdu_length(length.field()).set_next_link(match next {
                    MacHeTbNextLink::Link(link) => Bits::from(link),
                    MacHeTbNextLink::End => {
                        oer_chips_esp32s31_pac::wifi_mac::he_scratch::NextLink::End.into()
                    }
                });
            });
        if next == MacHeTbNextLink::End {
            self.mac
                .wifi_mac_tx_queue_vector
                .modify_he_mpdu_length_tail(reservation_bank(reservation), |w| {
                    w.set_link_index(Bits::from(index));
                });
        }
    }

    /// Remove Trigger eligibility after the queue has returned to software.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_tx_clr_mplen`, size `0x1aa`. The hardware-visible cleanup
    /// clears only `TB_ENA`; software-owned allocation bookkeeping is
    /// unnecessary for the deterministic disjoint reservation.
    pub fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        self.mac.wifi_mac_he_init_suffix.modify_queue_control(
            trigger_queue_element(reservation.queue),
            |w| {
                w.set_trigger_based_enable(false);
            },
        );
    }

    /// Read back the exact queue-control, MPLEN and BSR state just published
    /// by [`Self::prepare_he_trigger_based_queue`].
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_set_tb,
    /// libpp[hal_mac_tx.o]::mac_tx_set_mplen`, `hal_debug.o::{dbg_read_txq_conf1,
    /// dbg_read_tx_mplen,dbg_read_bsr_info,dbg_read_muedca_timer}` and their
    /// rev0 ROM equivalents. The reservation supplies the only indices; no
    /// raw address or unchecked queue number escapes this API.
    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> MacHeTriggerTxQueueSnapshot {
        let queue = reservation.queue;
        let suffix = &self.mac.wifi_mac_he_init_suffix;
        let queue_control = suffix.read_queue_control(trigger_queue_element(queue));
        let timer = self
            .mac
            .wifi_mac_he_mu_edca_timer
            .read_timer(element(queue.get()));
        let first = suffix.read_he_scratch(reservation.first);
        let tail = self
            .mac
            .wifi_mac_tx_queue_vector
            .read_he_mpdu_length_tail(reservation_bank(reservation));
        let status = &self.mac.wifi_mac_he_buffer_status;
        let bsr = status.read_software_bsr(element(queue.get()));
        let control = status.read_he_buffer_status_control();

        MacHeTriggerTxQueueSnapshot {
            logical_queue: reservation.queue.get().to_le_bytes()[0],
            tid: byte(queue_control.tid()),
            trigger_based_enabled: queue_control.trigger_based_enable(),
            mu_edca_timer_select: byte(queue_control.mu_edca_timer_select()),
            mu_edca_timer_enabled: timer.enable(),
            first_link: byte(queue_control.mpdu_length_link_address()),
            first_mpdu_length: halfword(first.mpdu_length()),
            first_next_link: byte(first.next_link()),
            tail_link: byte(tail.link_index()),
            programmed_msdu_bytes: bsr.value().get(),
            queued_msdu_bytes: bsr.value().get(),
            queue_valid: match queue.get() {
                0 => control.valid_0(),
                1 => control.valid_1(),
                2 => control.valid_2(),
                _ => control.valid_3(),
            },
        }
    }

    /// Select whether one TID participates in HE trigger-based BSR handling.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_tid_bitmap` and `hal_he_clr_tid_bitmap`, reached by the
    /// complete HE BlockAck response/deletion paths in
    /// `libnet80211.a[ieee80211_ht.o]`. The separate initial read and
    /// fresh read inside `modify` preserve the two-read/one-write blob edge.
    pub fn set_he_trigger_based_tid_enabled(
        &mut self,
        tid: MacHeTid,
        state: oer_chips_esp32s31_pac::wifi_mac::dictionaries::TriggerBasedTid,
    ) {
        let control = &mut self.mac.wifi_mac_he_buffer_status;
        let _initial = control.read_he_buffer_status_control();
        control.modify_he_buffer_status_control(|w| {
            match tid.value() {
                0 => w.set_trigger_based_tid_0_enabled(state),
                1 => w.set_trigger_based_tid_1_enabled(state),
                2 => w.set_trigger_based_tid_2_enabled(state),
                3 => w.set_trigger_based_tid_3_enabled(state),
                4 => w.set_trigger_based_tid_4_enabled(state),
                5 => w.set_trigger_based_tid_5_enabled(state),
                6 => w.set_trigger_based_tid_6_enabled(state),
                // MacHeTid holds 0 to 7.
                _ => w.set_trigger_based_tid_7_enabled(state),
            };
        });
    }

    /// Make trigger-based transmissions respect or ignore CCA and NAV.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_tb_ignore_cca_enable` and its own strings. A set hardware bit means
    /// "tb care CCA and NAV"; the debug ignore mode clears it.
    pub fn set_he_trigger_based_cca_and_nav_care(
        &mut self,
        care: oer_chips_esp32s31_pac::wifi_mac::ersu_and_vht_control::TbCareCcaAndNav,
    ) {
        self.mac
            .wifi_mac_he_init_suffix
            .modify_ersu_and_vht_control(|w| {
                w.set_tb_care_cca_and_nav(care);
            });
    }

    /// Apply the complete OBSS narrow-band RU aggregate disable leaf.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_disable_obss_narrow_bw_ru`, size `0x3e`. The whole-leaf meaning
    /// is symbol-exact; the individual twenty-bit bitmap and five-bit class
    /// identities remain approximate. The bitmap's store sets its one
    /// field; the bits above it are kept.
    pub fn disable_he_obss_narrow_band_ru(&mut self) {
        let registers = &mut self.mac.wifi_mac_he_obss_narrow_band_ru;
        registers.modify_disable_bitmap(|w| {
            w.set_value(Bits::constant::<0x000f_ffff>());
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_class_disable(oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::ClassDisable::AllDisabled.into());
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_global_disable(oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::GlobalDisable::Disabled);
        });
    }

    /// Undo [`Self::disable_he_obss_narrow_band_ru`]: the same three
    /// stores with every value cleared.
    pub fn enable_he_obss_narrow_band_ru(&mut self) {
        let registers = &mut self.mac.wifi_mac_he_obss_narrow_band_ru;
        registers.modify_disable_bitmap(|w| {
            w.set_value(Bits::constant::<0>());
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_class_disable(oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::ClassDisable::AllEnabled.into());
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_global_disable(oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::GlobalDisable::Enabled);
        });
    }
}

#[cfg(test)]
mod tests;
