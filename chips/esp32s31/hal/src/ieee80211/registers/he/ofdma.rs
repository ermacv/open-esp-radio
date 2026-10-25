//! Typed HE trigger/OFDMA control and best-effort hardware diagnostics.

use oer_mmio::{Bits, Index};

use crate::ieee80211::registers::{
    MacPlacement, ORDINARY_QUEUE_COUNT, WifiRadioRegisters, bits, byte, element, halfword,
    ordinary_bank,
};
const MPDU_LENGTH_LINK_END: u8 = 0x7f;

/// One IEEE 802.11 traffic identifier accepted by the recovered HE BSR block.
///
/// Keeping this as a bounded newtype means array and MMIO indexing cannot
/// receive an arbitrary `u8`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTid(u8);

impl MacHeTid {
    pub const COUNT: usize = 8;

    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT as u8 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    const fn mask(self) -> u8 {
        1 << self.0
    }
}

/// Configured maximum number of TIDs eligible for HE trigger-based TX.
///
/// SOURCE: complete `libpp[if_hecfg.o]::wifi_he_get_hetb_tid_bitmap` and its four-byte table. The enum is the
/// Rust-owned replacement for `g_wifi_menuconfig + 0x5c`; callers cannot
/// manufacture the blob's invalid configuration values.
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTbLinkReservation {
    queue: u8,
    first: u8,
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
        if queue >= ORDINARY_QUEUE_COUNT || count == 0 {
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
        Some(Self {
            queue,
            first,
            count,
        })
    }

    pub const fn queue(self) -> u8 {
        self.queue
    }

    pub const fn first(self) -> u8 {
        self.first
    }

    pub const fn count(self) -> u8 {
        self.count
    }

    const fn index(self, position: u8) -> Option<u8> {
        if position < self.count {
            Some(self.first + position)
        } else {
            None
        }
    }

    const fn next(self, position: u8) -> Option<u8> {
        if position >= self.count {
            None
        } else if position + 1 == self.count {
            Some(MPDU_LENGTH_LINK_END)
        } else {
            Some(self.first + position + 1)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHeTbProgramError {
    TidNotEligible,
    LengthCountMismatch,
    InvalidMpduLength,
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

/// One non-latched view of all hardware-visible HE Buffer Status Reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeBufferStatusSnapshot {
    pub hardware: [u32; MacHeTid::COUNT],
    pub software: [u32; MacHeTid::COUNT],
    pub valid: [bool; MacHeTid::COUNT],
    pub trigger_based_tid_enabled: [bool; MacHeTid::COUNT],
    pub ac_empty_software_tid: u8,
    pub ac_empty_uses_software_tid: bool,
    pub basic_special_bsr_sequence: bool,
    pub tid_limit_zero_bsr_sequence: bool,
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

/// WDEVTXQ_CONF1 fields for one of the eight reverse-addressed logical queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTriggerQueueConfiguration {
    pub tid: u8,
    pub trigger_based_enabled: bool,
    pub mu_edca_timer_select: u8,
    pub mpdu_length_link_address: u8,
    pub minimum_tx_power: u8,
}

/// WDEVTXQ_CONF2 fields for one of the four ordinary EDCA queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeEdcaQueueConfiguration {
    pub minimum_mpdu_length_cbw20: u16,
    pub minimum_mpdu_length_cbw40: u16,
    pub minimum_mpdu_length_cbw80: u16,
    pub software_rts: bool,
    pub software_cts: bool,
}

/// One MU-EDCA timer decoded in the blob's eight-TU units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeMuEdcaTimerSnapshot {
    pub timer_8tu: u8,
    pub enabled: bool,
    pub reset: bool,
    pub current_count_8tu: u8,
    pub reached: bool,
    pub aifs: u8,
}

/// Fixed-allocation snapshot of all recovered HE queue scheduling fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeQueueSchedulingSnapshot {
    pub trigger_queues: [MacHeTriggerQueueConfiguration; 8],
    pub edca_queues: [MacHeEdcaQueueConfiguration; 4],
    pub mu_edca_timers: [MacHeMuEdcaTimerSnapshot; 4],
}

/// Receive power-save fields decoded by `dbg_read_rx_misc`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeRxPowerSaveSnapshot {
    pub threshold: u16,
    pub enabled: bool,
    pub stop_rf: bool,
    pub ready: bool,
    pub front_end_frequency_hop_time: u8,
    pub phy_signal_delay: u8,
    pub intra_bss_color_check_enabled: bool,
    pub intra_ppdu_enabled: bool,
    pub vht_txop_address_check_enabled: bool,
    pub vht_txop_enabled: bool,
}

/// One custom receive frame-type matcher and its automatic-ACK selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeCustomReceiveType {
    pub enabled: bool,
    pub value: u8,
    pub ack_index: u8,
}

/// Receive beamforming state decoded from `WDEVBEAMFORMCONF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeBeamformingConfigurationSnapshot {
    pub memory_write_enabled: bool,
    pub bfrp_time: u8,
    pub ndp_time: u8,
    pub hardware_sequence_select: u8,
    pub hardware_sequence_enabled: bool,
    pub he_beam_enabled: bool,
    pub non_trigger_based_ru_select: bool,
    pub ru_select: bool,
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
            is_lower_bound: raw_code == 0x7f,
        }
    }
}

/// Allocation-free view of the HE/RX policy words decoded by
/// `dbg_read_rx_misc`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeReceiveConfigurationSnapshot {
    pub bss_color: u8,
    pub bss_color_enabled: bool,
    pub partial_bss_color_enabled: bool,
    pub bssid_select: u8,
    pub he_bssid_enabled: bool,
    pub multi_bssid_enabled: bool,
    pub multi_bssid_mask: u8,
    pub co_hosted_enabled: bool,
    pub default_packet_extension_duration: u8,
    pub mpdu_length_offset: u16,
    pub nfrp_buffer_threshold: u8,
    pub hardware_txop_enabled: bool,
    pub bsr_update_enabled: bool,
    pub trigger_based_stop_option: bool,
    pub trigger_response_scheduling_supported: bool,
    pub uplink_mu_data_disabled: bool,
    pub uplink_mu_disabled: bool,
    pub trigger_based_no_resource_continue_tx: bool,
    pub automatic_ack_allows_extended_range_su: bool,
    pub he_response_ack: bool,
    pub nominal_packet_padding_duration: [u8; 5],
    pub rx_control_9_bssid_position: u8,
    pub power_save: MacHeRxPowerSaveSnapshot,
    /// Profiles are ordered exactly as CUSTOM_TYPE0, 1 and 2.
    pub custom_receive_types: [MacHeCustomReceiveType; 3],
    pub beamforming: MacHeBeamformingConfigurationSnapshot,
}

/// The CONF1 element of logical trigger queue `queue`: the eight physical
/// words are addressed in reverse logical order.
const fn trigger_queue_element(queue: u8) -> Index<8> {
    element(7 - queue as usize)
}

/// The physical bank of a reservation's ordinary queue.
const fn reservation_bank(reservation: MacHeTbLinkReservation) -> Index<4> {
    match ordinary_bank(reservation.queue) {
        Some(bank) => bank,
        None => panic!("a reservation owns an ordinary queue"),
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
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        if !policy.contains(tid) {
            return Err(MacHeTbProgramError::TidNotEligible);
        }
        if mpdu_lengths.len() != usize::from(reservation.count) {
            return Err(MacHeTbProgramError::LengthCountMismatch);
        }
        if mpdu_lengths
            .iter()
            .any(|length| *length == 0 || *length > 0x3fff)
        {
            return Err(MacHeTbProgramError::InvalidMpduLength);
        }
        if queued_msdu_bytes > 0x000f_ffff {
            return Err(MacHeTbProgramError::QueuedBytesTooLarge);
        }

        let queue = reservation.queue;
        let physical_queue = trigger_queue_element(queue);

        // mac_tx_set_tb enables the selected MU-EDCA timer before allocating
        // the first MPDU-length entry.
        self.mac
            .wifi_mac_he_mu_edca_timer
            .modify_timer(element(usize::from(queue)), |w| {
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
                .set_mu_edca_timer_select(bits(u32::from(queue)))
                .set_mpdu_length_link_address(bits(u32::from(reservation.first)))
                .set_tid(bits(u32::from(tid.value())))
                .set_minimum_tx_power(Bits::constant::<0>())
                .set_high_cleared_by_tb_setup(Bits::constant::<0>());
        });

        for (position, length) in mpdu_lengths.iter().enumerate().skip(1) {
            let position =
                u8::try_from(position).expect("a reservation holds at most 120 positions");
            self.write_he_mpdu_length_link(reservation, position, *length);
        }

        let status = &mut self.mac.wifi_mac_he_buffer_status;
        status.modify_software_bsr(element(usize::from(queue)), |w| {
            w.set_value(bits(queued_msdu_bytes));
        });
        // Do not add a fence or diagnostic read here. The complete blob and
        // ROM sequence is exactly SW(BSR), LW(CONTROL), SW(CONTROL). BSR
        // hardware update is live, so lengthening the interval before the
        // validity RMW changes observable behavior.
        status.modify_he_buffer_status_control(|w| {
            match queue {
                0 => w.set_valid_0(true),
                1 => w.set_valid_1(true),
                2 => w.set_valid_2(true),
                3 => w.set_valid_3(true),
                _ => unreachable!("ordinary queue validation precedes publication"),
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
        length: u16,
    ) {
        let index = reservation
            .index(position)
            .expect("each linked position lies inside the reservation");
        let next = reservation
            .next(position)
            .expect("each linked position lies inside the reservation");
        self.mac
            .wifi_mac_he_init_suffix
            .modify_he_scratch(element(usize::from(index)), |w| {
                w.set_mpdu_length(bits(u32::from(length)))
                    .set_next_link(bits(u32::from(next)));
            });
        if next == MPDU_LENGTH_LINK_END {
            self.mac
                .wifi_mac_tx_queue_vector
                .modify_he_mpdu_length_tail(reservation_bank(reservation), |w| {
                    w.set_link_index(bits(u32::from(index)));
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
            .read_timer(element(usize::from(queue)));
        let first = suffix.read_he_scratch(element(usize::from(reservation.first)));
        let tail = self
            .mac
            .wifi_mac_tx_queue_vector
            .read_he_mpdu_length_tail(reservation_bank(reservation));
        let status = &self.mac.wifi_mac_he_buffer_status;
        let bsr = status.read_software_bsr(element(usize::from(queue)));
        let control = status.read_he_buffer_status_control();

        MacHeTriggerTxQueueSnapshot {
            logical_queue: queue,
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
            queue_valid: match queue {
                0 => control.valid_0(),
                1 => control.valid_1(),
                2 => control.valid_2(),
                3 => control.valid_3(),
                _ => unreachable!("reservation owns an ordinary queue"),
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

    /// Read all eight interleaved hardware/software BSR values and control.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_bsr_info` and `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_get_hw_txq_bsr`.
    pub fn he_buffer_status_snapshot(&self) -> MacHeBufferStatusSnapshot {
        let registers = &self.mac.wifi_mac_he_buffer_status;
        let mut hardware = [0_u32; MacHeTid::COUNT];
        let mut software = [0_u32; MacHeTid::COUNT];
        for tid in 0..MacHeTid::COUNT {
            hardware[tid] = registers.read_hardware_bsr(element(tid)).value().get();
            software[tid] = registers.read_software_bsr(element(tid)).value().get();
        }
        let control = registers.read_he_buffer_status_control();

        MacHeBufferStatusSnapshot {
            hardware,
            software,
            valid: [
                control.valid_0(),
                control.valid_1(),
                control.valid_2(),
                control.valid_3(),
                control.valid_4(),
                control.valid_5(),
                control.valid_6(),
                control.valid_7(),
            ],
            trigger_based_tid_enabled: [
                control.trigger_based_tid_0_enabled(),
                control.trigger_based_tid_1_enabled(),
                control.trigger_based_tid_2_enabled(),
                control.trigger_based_tid_3_enabled(),
                control.trigger_based_tid_4_enabled(),
                control.trigger_based_tid_5_enabled(),
                control.trigger_based_tid_6_enabled(),
                control.trigger_based_tid_7_enabled(),
            ],
            ac_empty_software_tid: byte(control.ac_empty_software_tid()),
            ac_empty_uses_software_tid: control.ac_empty_use_software_tid(),
            basic_special_bsr_sequence: control.basic_special_bsr_sequence(),
            tid_limit_zero_bsr_sequence: control.tid_limit_zero_bsr_sequence(),
        }
    }

    /// Read all HE/RX configuration fields recovered from `dbg_read_rx_misc`.
    ///
    /// This is a best-effort multi-register sample. It deliberately exposes
    /// the blob's hardware polarity: `uplink_mu_disabled == true` means the
    /// disable bit is set, while `hardware_txop_enabled == true` means the
    /// blob's `HE_USE_SW_TXOP` bit is clear.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_rx_misc`, size `0x42a`, and its function-local strings.
    pub fn he_receive_configuration_snapshot(&self) -> MacHeReceiveConfigurationSnapshot {
        let suffix = &self.mac.wifi_mac_he_init_suffix;
        let multi = suffix.read_multi_bssid_control();
        let options1 = suffix.read_ersu_and_vht_control();
        let options2 = suffix.read_he_default_control();
        let padding = suffix.read_he_packet_padding();
        let power_save = self.mac.wifi_mac_rx_power_save.read_rx_power_save_control();
        let custom = self
            .mac
            .wifi_mac_rx_custom_type
            .read_rx_custom_type_control();
        let beamforming = self.mac.wifi_mac_he_init_prefix.read_bf_timing_control();

        MacHeReceiveConfigurationSnapshot {
            bss_color: byte(multi.bss_color()),
            bss_color_enabled: multi.bss_color_enable(),
            partial_bss_color_enabled: multi.partial_bss_color_enable(),
            bssid_select: byte(multi.bssid_select()),
            he_bssid_enabled: multi.he_bssid_enable(),
            multi_bssid_enabled: multi.multi_bssid_enable(),
            multi_bssid_mask: multi.multi_bssid_mask(),
            co_hosted_enabled: multi.co_hosted_enable(),
            default_packet_extension_duration: byte(options2.default_pe_duration()),
            mpdu_length_offset: halfword(options2.mpdu_length_offset()),
            nfrp_buffer_threshold: options2.nfrp_buffer_threshold(),
            hardware_txop_enabled: !options2.he_use_software_txop(),
            bsr_update_enabled: options2.bsr_update_enable(),
            trigger_based_stop_option: options2.tb_stop_option(),
            trigger_response_scheduling_supported: options2.he_trs_support(),
            uplink_mu_data_disabled: options2.ul_mu_data_disable(),
            uplink_mu_disabled: options2.ul_mu_disable(),
            trigger_based_no_resource_continue_tx: options1.tb_no_resource_continue_tx(),
            automatic_ack_allows_extended_range_su: options1.auto_ack_allow_ersu(),
            he_response_ack: options1.he_response_ack(),
            nominal_packet_padding_duration: [
                byte(padding.bpsk_duration()),
                byte(padding.qpsk_duration()),
                byte(padding.qam16_duration()),
                byte(padding.qam64_duration()),
                byte(padding.qam256_duration()),
            ],
            rx_control_9_bssid_position: byte(
                self.mac
                    .wifi_mac_rx_bssid_list
                    .read_rx_bssid_list_control()
                    .rx_control_9_bssid_position(),
            ),
            power_save: MacHeRxPowerSaveSnapshot {
                threshold: halfword(power_save.rx_ps_threshold()),
                enabled: power_save.rx_ps_enable(),
                stop_rf: power_save.stop_rf(),
                ready: power_save.ps_ready(),
                front_end_frequency_hop_time: power_save.fe_frequency_hop_time(),
                phy_signal_delay: byte(power_save.rx_phy_signal_delay()),
                intra_bss_color_check_enabled: power_save.intra_ps_check_bss_color_enable(),
                intra_ppdu_enabled: power_save.intra_ppdu_ps_enable(),
                vht_txop_address_check_enabled: power_save.rx_vht_txop_ps_check_address(),
                vht_txop_enabled: power_save.rx_vht_txop_ps_enable(),
            },
            custom_receive_types: [
                MacHeCustomReceiveType {
                    enabled: custom.type_0_enable(),
                    value: byte(custom.type_0_value()),
                    ack_index: byte(custom.type_0_ack_index()),
                },
                MacHeCustomReceiveType {
                    enabled: custom.type_1_enable(),
                    value: byte(custom.type_1_value()),
                    ack_index: byte(custom.type_1_ack_index()),
                },
                MacHeCustomReceiveType {
                    enabled: custom.type_2_enable(),
                    value: byte(custom.type_2_value()),
                    ack_index: byte(custom.type_2_ack_index()),
                },
            ],
            beamforming: MacHeBeamformingConfigurationSnapshot {
                memory_write_enabled: beamforming.bf_memory_write_enable(),
                bfrp_time: byte(beamforming.rx_bfrp_timeout_ms()),
                ndp_time: beamforming.he_beam_ndp_time(),
                hardware_sequence_select: byte(beamforming.he_beam_hw_sequence_select()),
                hardware_sequence_enabled: beamforming.he_beam_hw_sequence_enable(),
                he_beam_enabled: beamforming.he_beam_enable(),
                non_trigger_based_ru_select: beamforming.non_tb_beam_ru_select(),
                ru_select: beamforming.beam_ru_select(),
            },
        }
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
            w.set_value(bits(if disabled { 0x000f_ffff } else { 0 }));
        });
        registers.modify_he_obss_narrow_band_ru_control(|w| {
            w.set_class_disable(bits(if disabled { 0x1f } else { 0 }));
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

    /// Read queue-local TB, TID, minimum-MPDU and MU-EDCA state.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_txq_conf1`, `dbg_read_txq_conf2` and
    /// `dbg_read_muedca_timer`. CONF1 and CONF2 use reverse logical queue
    /// addressing, normalized here so array index equals the blob's logged
    /// logical queue number.
    pub fn he_queue_scheduling_snapshot(&self) -> MacHeQueueSchedulingSnapshot {
        let suffix = &self.mac.wifi_mac_he_init_suffix;
        let queue_control = &self.mac.wifi_mac_tx_queue_control;
        let timers = &self.mac.wifi_mac_he_mu_edca_timer;

        let trigger_queues = core::array::from_fn(|logical_queue| {
            let queue = suffix.read_queue_control(element(7 - logical_queue));
            MacHeTriggerQueueConfiguration {
                tid: byte(queue.tid()),
                trigger_based_enabled: queue.trigger_based_enable(),
                mu_edca_timer_select: byte(queue.mu_edca_timer_select()),
                mpdu_length_link_address: byte(queue.mpdu_length_link_address()),
                minimum_tx_power: byte(queue.minimum_tx_power()),
            }
        });
        let edca_queues = core::array::from_fn(|logical_queue| {
            let queue = queue_control.read_protection(element(3 - logical_queue));
            MacHeEdcaQueueConfiguration {
                minimum_mpdu_length_cbw20: halfword(queue.minimum_mpdu_length_cbw20()),
                minimum_mpdu_length_cbw40: halfword(queue.minimum_mpdu_length_cbw40()),
                minimum_mpdu_length_cbw80: halfword(queue.minimum_mpdu_length_cbw80()),
                software_rts: queue.software_rts(),
                software_cts: queue.software_cts(),
            }
        });
        let mu_edca_timers = core::array::from_fn(|index| {
            let timer = timers.read_timer(element(index));
            MacHeMuEdcaTimerSnapshot {
                timer_8tu: timer.timer_8tu(),
                enabled: timer.enable(),
                reset: timer.reset(),
                current_count_8tu: timer.current_count_8tu(),
                reached: timer.reached(),
                aifs: byte(timer.aifs()),
            }
        });

        MacHeQueueSchedulingSnapshot {
            trigger_queues,
            edca_queues,
            mu_edca_timers,
        }
    }
}

#[cfg(test)]
mod tests;
