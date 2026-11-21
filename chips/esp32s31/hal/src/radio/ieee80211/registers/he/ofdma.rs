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
    pub const fn for_queue(policy: MacHeTbTidLimit, queue: u8, count: u8) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let (first, last) = match (policy, queue) {
            (MacHeTbTidLimit::One, 0) => (64, 81),
            (MacHeTbTidLimit::One, 1) => (82, 99),
            (MacHeTbTidLimit::One, 2) => (0, 63),
            (MacHeTbTidLimit::One, 3) => (100, 117),
            (_, 0) => (32, 63),
            (_, 1) => (64, 95),
            (_, 2) => (0, 31),
            (_, 3) => (96, 119),
            _ => return None,
        };
        if count > last - first + 1 {
            return None;
        }
        let (Ok(queue), Ok(first)) = (Index::new(queue as usize), Index::new(first as usize))
        else {
            return None;
        };
        Some(Self {
            queue,
            first,
            count,
        })
    }

    pub const fn queue(self) -> u8 {
        // Below four: the low byte is the whole index.
        self.queue.get().to_le_bytes()[0]
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

/// Key fields from the last HE Trigger receive calculation.
///
/// The blob explicitly warns that `WDEVAXDIAG0..9` can be overwritten by a
/// following RX frame. This is a diagnostic sample, not durable protocol
/// state. Pair it with a change in `MacHeTbStatistics::rx_trigger_count`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTriggerRxDiagnostics {
    pub trigger_state_cs: u8,
    pub station_match: bool,
    pub trigger_type: u8,
    pub uplink_length: u16,
    pub association_id: u16,
    pub ru_allocation_raw: u8,
    pub ru_allocation: u8,
    pub uplink_mcs: u8,
    pub uplink_dcm: bool,
    pub fec_coding_type: bool,
    pub gi_and_ltf_type: u8,
    pub uplink_bandwidth: u8,
    pub stbc: bool,
    pub mu_mimo_ltf: bool,
    pub spatial_reuse: u16,
    pub basic_target_rssi: u8,
    pub spatial_stream_allocation: u8,
    pub cs_required: bool,
    pub more_trigger_frames: bool,
    pub symbol_count: u16,
    pub tb_apep_length: u16,
    pub cbw20_packet_count: u8,
    pub cbw40_packet_count: u8,
    pub cbw80_packet_count: u8,
    pub qos_null_append_packet_count: u8,
    pub ax_match_bssid1: bool,
    pub ax_match_bssid0: bool,
    pub bss_color_valid: bool,
    pub rx_ampdu_auto_ack_valid: bool,
}

/// Non-latched hardware state for the HE sounding/feedback sequence.
///
/// Complete `dbg_read_ax_diag` prints these fields from `WDEVAXDIAG0` and
/// `WDEVAXDIAG3`. Like the Trigger fields in the same words, a following RX
/// frame may overwrite them. Sample this immediately after a parsed NDPA/NDP
/// edge; it is evidence about hardware progress, not owned protocol state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeBeamformingDiagnostics {
    pub transmit_or_receive_beam: bool,
    pub bfrp_timer: u8,
    pub bfrp_detection_enabled: bool,
    pub beam_timer: u8,
    pub ndp_detected_successfully: bool,
    pub feedback_status: bool,
}

/// Stream-zero average SNR captured by the beamforming-report hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacBeamformingAverageSnr {
    /// Raw byte printed by the complete vendor debug helper.
    pub raw_code: u8,
    /// SNR in quarter-dB units: `(raw_code + 88)`.
    pub quarter_db: u16,
    /// Code `0x7f` is rendered by the blob as a lower bound (`>=`).
    pub is_lower_bound: bool,
}

impl MacBeamformingAverageSnr {
    const fn from_raw_code(raw_code: u8) -> Self {
        Self {
            raw_code,
            quarter_db: raw_code as u16 + 88,
            is_lower_bound: raw_code
                == oer_chips_esp32s31_pac::wifi_mac::average_snr::Stream0Code::SaturatedOrInvalid
                    .bits() as u8,
        }
    }
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
    /// Read the beamforming-report average SNR without using floating point.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::esp_test_get_bfr_avgsnr`, size `0x86`. The blob reads
    /// `0x20105f94[7:0]` and computes `(code + 88.0) * 0.25` dB. Code `0x7f`
    /// uses a `>=` prefix, so the typed value retains that saturation state.
    pub fn beamforming_average_snr(&self) -> MacBeamformingAverageSnr {
        let raw_code = self
            .mac
            .wifi_mac_beamforming_report
            .read_average_snr()
            .stream_0_code();
        MacBeamformingAverageSnr::from_raw_code(raw_code)
    }

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
            logical_queue: reservation.queue(),
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
    pub fn set_he_trigger_based_tid_enabled(&mut self, tid: MacHeTid, enabled: bool) {
        let control = &mut self.mac.wifi_mac_he_buffer_status;
        let _initial = control.read_he_buffer_status_control();
        control.modify_he_buffer_status_control(|w| {
            match tid.value() {
                0 => w.set_trigger_based_tid_0_enabled(enabled),
                1 => w.set_trigger_based_tid_1_enabled(enabled),
                2 => w.set_trigger_based_tid_2_enabled(enabled),
                3 => w.set_trigger_based_tid_3_enabled(enabled),
                4 => w.set_trigger_based_tid_4_enabled(enabled),
                5 => w.set_trigger_based_tid_5_enabled(enabled),
                6 => w.set_trigger_based_tid_6_enabled(enabled),
                7 => w.set_trigger_based_tid_7_enabled(enabled),
                _ => unreachable!("MacHeTid is bounded"),
            };
        });
    }

    /// Make trigger-based transmissions respect or ignore CCA and NAV.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_tb_ignore_cca_enable` and its own strings. A set hardware bit means
    /// "tb care CCA and NAV"; the debug ignore mode clears it.
    pub fn set_he_trigger_based_cca_and_nav_care(&mut self, care: bool) {
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
    pub fn set_he_obss_narrow_band_ru_disabled(&mut self, disabled: bool) {
        let registers = &mut self.mac.wifi_mac_he_obss_narrow_band_ru;
        registers.modify_disable_bitmap(|w| {
            w.set_value(if disabled {
                Bits::constant::<0x000f_ffff>()
            } else {
                Bits::constant::<0>()
            });
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_class_disable(
                if disabled {
                    oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::ClassDisable::AllDisabled
                } else {
                    oer_chips_esp32s31_pac::wifi_mac::he_obss_narrow_band_ru_control::ClassDisable::AllEnabled
                }
                .into(),
            );
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_global_disable(disabled);
        });
    }

    /// Read key fields from the blob's ten-word Trigger RX diagnostic block.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_ax_diag`, size `0x466`, and its `WDEVAXDIAG0..9` strings.
    pub fn he_trigger_receive_diagnostics(&self) -> MacHeTriggerRxDiagnostics {
        let registers = &self.mac.wifi_mac_he_trigger_rx_diagnostics;
        let state = registers.read_he_trigger_rx_diagnostics_state();
        let length = registers.read_length();
        let dependent = registers.read_trigger_dependent();
        let user = registers.read_basic_user();
        let phy = registers.read_common_phy();
        let trigger = registers.read_common_trigger();
        let counts = registers.read_packet_counts();
        let ru_allocation_raw = user.ru_allocation_raw();

        MacHeTriggerRxDiagnostics {
            trigger_state_cs: byte(state.trigger_state_cs()),
            station_match: dependent.sta_match(),
            trigger_type: byte(trigger.trigger_type()),
            uplink_length: halfword(trigger.ul_length()),
            association_id: halfword(user.rx_aid()),
            ru_allocation_raw,
            ru_allocation: ru_allocation_raw >> 1,
            uplink_mcs: byte(user.ul_mcs()),
            uplink_dcm: dependent.basic_ul_dcm(),
            fec_coding_type: user.fec_coding_type(),
            gi_and_ltf_type: byte(phy.gi_and_ltf_type()),
            uplink_bandwidth: byte(phy.ul_bandwidth()),
            stbc: phy.stbc(),
            mu_mimo_ltf: phy.mu_mimo_ltf(),
            spatial_reuse: phy.spatial_reuse(),
            basic_target_rssi: byte(dependent.basic_target_rssi()),
            spatial_stream_allocation: byte(dependent.basic_spatial_stream_allocation()),
            cs_required: trigger.cs_required(),
            more_trigger_frames: trigger.more_tf(),
            symbol_count: length.symbol_count(),
            tb_apep_length: length.tb_apep_length(),
            cbw20_packet_count: byte(counts.cbw20_packet_count()),
            cbw40_packet_count: byte(counts.cbw40_packet_count()),
            cbw80_packet_count: byte(counts.cbw80_packet_count()),
            qos_null_append_packet_count: byte(counts.qos_null_append_packet_count()),
            ax_match_bssid1: counts.ax_match_bssid1(),
            ax_match_bssid0: counts.ax_match_bssid0(),
            bss_color_valid: counts.bss_color_valid(),
            rx_ampdu_auto_ack_valid: counts.rx_ampdu_auto_ack_valid(),
        }
    }

    /// Read the hardware-owned HE NDP/feedback sequence state.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_ax_diag`, size `0x466`, and its exact `WDEVAXDIAG0` and
    /// `WDEVAXDIAG3` format strings. Complete
    /// `libpp[wdev.o]::wDev_ProcessRxSucData`, size `0x6ca`, only
    /// logs the result of `libpp[wdev.o]::is_ndpa_to_dut`; it does not call a software
    /// feedback producer. These diagnostic bits therefore observe the
    /// hardware sequence configured by `hal_init_bf`.
    pub fn he_beamforming_diagnostics(&self) -> MacHeBeamformingDiagnostics {
        let registers = &self.mac.wifi_mac_he_trigger_rx_diagnostics;
        let state = registers.read_he_trigger_rx_diagnostics_state();
        let padding = registers.read_padding();

        MacHeBeamformingDiagnostics {
            transmit_or_receive_beam: state.txrx_beam(),
            bfrp_timer: byte(state.bfrp_timer()),
            bfrp_detection_enabled: state.bfrp_detect_enable(),
            beam_timer: byte(state.beam_timer()),
            ndp_detected_successfully: state.ndp_detect_success(),
            feedback_status: padding.feedback_status(),
        }
    }
}

#[cfg(test)]
mod tests;
