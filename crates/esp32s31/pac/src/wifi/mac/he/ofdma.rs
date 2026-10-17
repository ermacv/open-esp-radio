//! Typed HE trigger/OFDMA control and best-effort hardware diagnostics.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

const ORDINARY_QUEUE_COUNT: u8 = 4;
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

impl WifiRadioRegisters {
    /// Read the beamforming-report average SNR without using floating point.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::esp_test_get_bfr_avgsnr`, size `0x86`. The blob reads
    /// `0x20105f94[7:0]` and computes `(code + 88.0) * 0.25` dB. Code `0x7f`
    /// uses a `>=` prefix, so the typed value retains that saturation state.
    pub fn beamforming_average_snr(&self) -> MacBeamformingAverageSnr {
        let raw_code = self
            .peripherals
            .wifi_mac
            .wifi_mac_beamforming_report
            .average_snr()
            .read()
            .stream_0_code()
            .bits();
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
        let physical_queue = 7 - usize::from(queue);
        let vector_bank = 3 - usize::from(queue);
        let suffix = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        let timer = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_mu_edca_timer
            .timer(usize::from(queue));

        // mac_tx_set_tb enables the selected MU-EDCA timer before allocating
        // the first MPDU-length entry.
        timer.modify(|_, w| w.enable().set_bit());

        let write_link = |position: u8, length: u16| {
            let index = reservation.index(position).unwrap();
            let next = reservation.next(position).unwrap();
            suffix
                .he_scratch(usize::from(index))
                .modify(|_, w| w.mpdu_length().set(length).next_link().set(next));
            if next == MPDU_LENGTH_LINK_END {
                self.peripherals
                    .wifi_mac
                    .wifi_mac_tx_queue_vector
                    .he_mpdu_length_tail(vector_bank)
                    .modify(|_, w| w.link_index().set(index));
            }
        };

        write_link(0, mpdu_lengths[0]);

        // The setter preserves exactly bit two of the old CONF1 image and
        // constructs every other named field from the current frame/queue.
        let queue_control = suffix.queue_control(physical_queue);
        let qos_null_to_translated_bss = queue_control.read().qos_null_to_translated_bss().bit();
        crate::svd::zero_based_field_write::mac_he_trigger_queue_control(
            suffix,
            physical_queue,
            qos_null_to_translated_bss,
            true,
            queue,
            reservation.first,
            tid.value(),
        );

        for (position, length) in mpdu_lengths.iter().enumerate().skip(1) {
            write_link(position as u8, *length);
        }

        self.peripherals
            .wifi_mac
            .wifi_mac_he_buffer_status
            .software_bsr(usize::from(queue))
            .modify(|_, w| w.value().set(queued_msdu_bytes));
        // Do not add a fence or diagnostic read here. The complete blob and
        // ROM sequence is exactly SW(BSR), LW(CONTROL), SW(CONTROL). BSR
        // hardware update is live, so lengthening the interval before the
        // validity RMW changes observable behavior.
        self.peripherals
            .wifi_mac
            .wifi_mac_he_buffer_status
            .control()
            .modify(|_, w| match queue {
                0 => w.valid_0().set_bit(),
                1 => w.valid_1().set_bit(),
                2 => w.valid_2().set_bit(),
                3 => w.valid_3().set_bit(),
                _ => unreachable!("ordinary queue validation precedes publication"),
            });
        let mut snapshot = self.he_trigger_based_queue_snapshot(reservation);
        snapshot.programmed_msdu_bytes = queued_msdu_bytes;
        Ok(snapshot)
    }

    /// Remove Trigger eligibility after the queue has returned to software.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_tx_clr_mplen`, size `0x1aa`. The hardware-visible cleanup
    /// clears only `TB_ENA`; software-owned allocation bookkeeping is
    /// unnecessary for the deterministic disjoint reservation.
    pub fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        let physical_queue = 7 - usize::from(reservation.queue);
        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .queue_control(physical_queue)
            .modify(|_, w| w.trigger_based_enable().clear_bit());
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
        let physical_queue = 7 - usize::from(queue);
        let vector_bank = 3 - usize::from(queue);
        let suffix = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        let queue_control = suffix.queue_control(physical_queue).read();
        let timer = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_mu_edca_timer
            .timer(usize::from(queue))
            .read();
        let first = suffix.he_scratch(usize::from(reservation.first)).read();
        let tail = self
            .peripherals
            .wifi_mac
            .wifi_mac_tx_queue_vector
            .he_mpdu_length_tail(vector_bank)
            .read();
        let bsr = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_buffer_status
            .software_bsr(usize::from(queue))
            .read();
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_buffer_status
            .control()
            .read();

        MacHeTriggerTxQueueSnapshot {
            logical_queue: queue,
            tid: queue_control.tid().bits(),
            trigger_based_enabled: queue_control.trigger_based_enable().bit(),
            mu_edca_timer_select: queue_control.mu_edca_timer_select().bits(),
            mu_edca_timer_enabled: timer.enable().bit(),
            first_link: queue_control.mpdu_length_link_address().bits(),
            first_mpdu_length: first.mpdu_length().bits(),
            first_next_link: first.next_link().bits(),
            tail_link: tail.link_index().bits(),
            programmed_msdu_bytes: bsr.value().bits(),
            queued_msdu_bytes: bsr.value().bits(),
            queue_valid: match queue {
                0 => control.valid_0().bit(),
                1 => control.valid_1().bit(),
                2 => control.valid_2().bit(),
                3 => control.valid_3().bit(),
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
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_buffer_status
            .control();
        let _initial = control.read();
        control.modify(|_, w| match tid.value() {
            0 => w.trigger_based_tid_0_enabled().bit(enabled),
            1 => w.trigger_based_tid_1_enabled().bit(enabled),
            2 => w.trigger_based_tid_2_enabled().bit(enabled),
            3 => w.trigger_based_tid_3_enabled().bit(enabled),
            4 => w.trigger_based_tid_4_enabled().bit(enabled),
            5 => w.trigger_based_tid_5_enabled().bit(enabled),
            6 => w.trigger_based_tid_6_enabled().bit(enabled),
            7 => w.trigger_based_tid_7_enabled().bit(enabled),
            _ => unreachable!("MacHeTid is bounded"),
        });
    }

    /// Read all eight interleaved hardware/software BSR values and control.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_bsr_info` and `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_get_hw_txq_bsr`.
    pub fn he_buffer_status_snapshot(&self) -> MacHeBufferStatusSnapshot {
        let registers = &self.peripherals.wifi_mac.wifi_mac_he_buffer_status;
        let mut hardware = [0_u32; MacHeTid::COUNT];
        let mut software = [0_u32; MacHeTid::COUNT];
        for tid in 0..MacHeTid::COUNT {
            hardware[tid] = registers.hardware_bsr(tid).read().value().bits();
            software[tid] = registers.software_bsr(tid).read().value().bits();
        }
        let control = registers.control().read();

        MacHeBufferStatusSnapshot {
            hardware,
            software,
            valid: [
                control.valid_0().bit(),
                control.valid_1().bit(),
                control.valid_2().bit(),
                control.valid_3().bit(),
                control.valid_4().bit(),
                control.valid_5().bit(),
                control.valid_6().bit(),
                control.valid_7().bit(),
            ],
            trigger_based_tid_enabled: [
                control.trigger_based_tid_0_enabled().bit(),
                control.trigger_based_tid_1_enabled().bit(),
                control.trigger_based_tid_2_enabled().bit(),
                control.trigger_based_tid_3_enabled().bit(),
                control.trigger_based_tid_4_enabled().bit(),
                control.trigger_based_tid_5_enabled().bit(),
                control.trigger_based_tid_6_enabled().bit(),
                control.trigger_based_tid_7_enabled().bit(),
            ],
            ac_empty_software_tid: control.ac_empty_software_tid().bits(),
            ac_empty_uses_software_tid: control.ac_empty_use_software_tid().bit(),
            basic_special_bsr_sequence: control.basic_special_bsr_sequence().bit(),
            tid_limit_zero_bsr_sequence: control.tid_limit_zero_bsr_sequence().bit(),
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
        let suffix = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        let multi = suffix.multi_bssid_control().read();
        let options1 = suffix.ersu_and_vht_control().read();
        let options2 = suffix.he_default_control().read();
        let padding = suffix.he_packet_padding().read();
        let power_save = self
            .peripherals
            .wifi_mac
            .wifi_mac_rx_power_save
            .control()
            .read();
        let custom = self
            .peripherals
            .wifi_mac
            .wifi_mac_rx_custom_type
            .control()
            .read();
        let beamforming = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_prefix
            .bf_timing_control()
            .read();

        MacHeReceiveConfigurationSnapshot {
            bss_color: multi.bss_color().bits(),
            bss_color_enabled: multi.bss_color_enable().bit(),
            partial_bss_color_enabled: multi.partial_bss_color_enable().bit(),
            bssid_select: multi.bssid_select().bits(),
            he_bssid_enabled: multi.he_bssid_enable().bit(),
            multi_bssid_enabled: multi.multi_bssid_enable().bit(),
            multi_bssid_mask: multi.multi_bssid_mask().bits(),
            co_hosted_enabled: multi.co_hosted_enable().bit(),
            default_packet_extension_duration: options2.default_pe_duration().bits(),
            mpdu_length_offset: options2.mpdu_length_offset().bits(),
            nfrp_buffer_threshold: options2.nfrp_buffer_threshold().bits(),
            hardware_txop_enabled: !options2.he_use_software_txop().bit(),
            bsr_update_enabled: options2.bsr_update_enable().bit(),
            trigger_based_stop_option: options2.tb_stop_option().bit(),
            trigger_response_scheduling_supported: options2.he_trs_support().bit(),
            uplink_mu_data_disabled: options2.ul_mu_data_disable().bit(),
            uplink_mu_disabled: options2.ul_mu_disable().bit(),
            trigger_based_no_resource_continue_tx: options1.tb_no_resource_continue_tx().bit(),
            automatic_ack_allows_extended_range_su: options1.auto_ack_allow_ersu().bit(),
            he_response_ack: options1.he_response_ack().bit(),
            nominal_packet_padding_duration: [
                padding.bpsk_duration().bits(),
                padding.qpsk_duration().bits(),
                padding.qam16_duration().bits(),
                padding.qam64_duration().bits(),
                padding.qam256_duration().bits(),
            ],
            rx_control_9_bssid_position: self
                .peripherals
                .wifi_mac
                .wifi_mac_rx_bssid_list
                .control()
                .read()
                .rx_control_9_bssid_position()
                .bits(),
            power_save: MacHeRxPowerSaveSnapshot {
                threshold: power_save.rx_ps_threshold().bits(),
                enabled: power_save.rx_ps_enable().bit(),
                stop_rf: power_save.stop_rf().bit(),
                ready: power_save.ps_ready().bit(),
                front_end_frequency_hop_time: power_save.fe_frequency_hop_time().bits(),
                phy_signal_delay: power_save.rx_phy_signal_delay().bits(),
                intra_bss_color_check_enabled: power_save.intra_ps_check_bss_color_enable().bit(),
                intra_ppdu_enabled: power_save.intra_ppdu_ps_enable().bit(),
                vht_txop_address_check_enabled: power_save.rx_vht_txop_ps_check_address().bit(),
                vht_txop_enabled: power_save.rx_vht_txop_ps_enable().bit(),
            },
            custom_receive_types: [
                MacHeCustomReceiveType {
                    enabled: custom.type_0_enable().bit(),
                    value: custom.type_0_value().bits(),
                    ack_index: custom.type_0_ack_index().bits(),
                },
                MacHeCustomReceiveType {
                    enabled: custom.type_1_enable().bit(),
                    value: custom.type_1_value().bits(),
                    ack_index: custom.type_1_ack_index().bits(),
                },
                MacHeCustomReceiveType {
                    enabled: custom.type_2_enable().bit(),
                    value: custom.type_2_value().bits(),
                    ack_index: custom.type_2_ack_index().bits(),
                },
            ],
            beamforming: MacHeBeamformingConfigurationSnapshot {
                memory_write_enabled: beamforming.bf_memory_write_enable().bit(),
                bfrp_time: beamforming.rx_bfrp_timeout_ms().bits(),
                ndp_time: beamforming.he_beam_ndp_time().bits(),
                hardware_sequence_select: beamforming.he_beam_hw_sequence_select().bits(),
                hardware_sequence_enabled: beamforming.he_beam_hw_sequence_enable().bit(),
                he_beam_enabled: beamforming.he_beam_enable().bit(),
                non_trigger_based_ru_select: beamforming.non_tb_beam_ru_select().bit(),
                ru_select: beamforming.beam_ru_select().bit(),
            },
        }
    }

    /// Make trigger-based transmissions respect or ignore CCA and NAV.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_tb_ignore_cca_enable` and its own strings. A set hardware bit means
    /// "tb care CCA and NAV"; the debug ignore mode clears it.
    pub fn set_he_trigger_based_cca_and_nav_care(&mut self, care: bool) {
        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .ersu_and_vht_control()
            .modify(|_, w| w.tb_care_cca_and_nav().bit(care));
    }

    /// Apply the complete OBSS narrow-band RU aggregate disable leaf.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_disable_obss_narrow_bw_ru`, size `0x3e`. The whole-leaf meaning
    /// is symbol-exact; the individual twenty-bit bitmap and five-bit class
    /// identities remain approximate and are marked as such in the SVD.
    pub fn set_he_obss_narrow_band_ru_disabled(&mut self, disabled: bool) {
        let registers = &self.peripherals.wifi_mac.wifi_mac_he_obss_narrow_band_ru;
        crate::svd::zero_based_field_write::mac_he_obss_narrow_band_ru_disable_bitmap(
            registers,
            if disabled { 0x000f_ffff } else { 0 },
        );
        registers
            .control()
            .modify(|_, w| w.class_disable().set(if disabled { 0x1f } else { 0 }));
        registers
            .control()
            .modify(|_, w| w.global_disable().bit(disabled));
    }

    /// Read key fields from the blob's ten-word Trigger RX diagnostic block.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_ax_diag`, size `0x466`, and its `WDEVAXDIAG0..9` strings.
    pub fn he_trigger_receive_diagnostics(&self) -> MacHeTriggerRxDiagnostics {
        let registers = &self.peripherals.wifi_mac.wifi_mac_he_trigger_rx_diagnostics;
        let state = registers.state().read();
        let length = registers.length().read();
        let dependent = registers.trigger_dependent().read();
        let user = registers.basic_user().read();
        let phy = registers.common_phy().read();
        let trigger = registers.common_trigger().read();
        let counts = registers.packet_counts().read();
        let ru_allocation_raw = user.ru_allocation_raw().bits();

        MacHeTriggerRxDiagnostics {
            trigger_state_cs: state.trigger_state_cs().bits(),
            station_match: dependent.sta_match().bit(),
            trigger_type: trigger.trigger_type().bits(),
            uplink_length: trigger.ul_length().bits(),
            association_id: user.rx_aid().bits(),
            ru_allocation_raw,
            ru_allocation: ru_allocation_raw >> 1,
            uplink_mcs: user.ul_mcs().bits(),
            uplink_dcm: dependent.basic_ul_dcm().bit(),
            fec_coding_type: user.fec_coding_type().bit(),
            gi_and_ltf_type: phy.gi_and_ltf_type().bits(),
            uplink_bandwidth: phy.ul_bandwidth().bits(),
            stbc: phy.stbc().bit(),
            mu_mimo_ltf: phy.mu_mimo_ltf().bit(),
            spatial_reuse: phy.spatial_reuse().bits(),
            basic_target_rssi: dependent.basic_target_rssi().bits(),
            spatial_stream_allocation: dependent.basic_spatial_stream_allocation().bits(),
            cs_required: trigger.cs_required().bit(),
            more_trigger_frames: trigger.more_tf().bit(),
            symbol_count: length.symbol_count().bits(),
            tb_apep_length: length.tb_apep_length().bits(),
            cbw20_packet_count: counts.cbw20_packet_count().bits(),
            cbw40_packet_count: counts.cbw40_packet_count().bits(),
            cbw80_packet_count: counts.cbw80_packet_count().bits(),
            qos_null_append_packet_count: counts.qos_null_append_packet_count().bits(),
            ax_match_bssid1: counts.ax_match_bssid1().bit(),
            ax_match_bssid0: counts.ax_match_bssid0().bit(),
            bss_color_valid: counts.bss_color_valid().bit(),
            rx_ampdu_auto_ack_valid: counts.rx_ampdu_auto_ack_valid().bit(),
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
        let registers = &self.peripherals.wifi_mac.wifi_mac_he_trigger_rx_diagnostics;
        let state = registers.state().read();
        let padding = registers.padding().read();

        MacHeBeamformingDiagnostics {
            transmit_or_receive_beam: state.txrx_beam().bit(),
            bfrp_timer: state.bfrp_timer().bits(),
            bfrp_detection_enabled: state.bfrp_detect_enable().bit(),
            beam_timer: state.beam_timer().bits(),
            ndp_detected_successfully: state.ndp_detect_success().bit(),
            feedback_status: padding.feedback_status().bit(),
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
        let suffix = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        let queue_control = &self.peripherals.wifi_mac.wifi_mac_tx_queue_control;
        let timers = &self.peripherals.wifi_mac.wifi_mac_he_mu_edca_timer;

        let trigger_queues = core::array::from_fn(|logical_queue| {
            let physical_queue = 7 - logical_queue;
            let queue = suffix.queue_control(physical_queue).read();
            MacHeTriggerQueueConfiguration {
                tid: queue.tid().bits(),
                trigger_based_enabled: queue.trigger_based_enable().bit(),
                mu_edca_timer_select: queue.mu_edca_timer_select().bits(),
                mpdu_length_link_address: queue.mpdu_length_link_address().bits(),
                minimum_tx_power: queue.minimum_tx_power().bits(),
            }
        });
        let edca_queues = core::array::from_fn(|logical_queue| {
            let physical_queue = 3 - logical_queue;
            let queue = queue_control.protection(physical_queue).read();
            MacHeEdcaQueueConfiguration {
                minimum_mpdu_length_cbw20: queue.minimum_mpdu_length_cbw20().bits(),
                minimum_mpdu_length_cbw40: queue.minimum_mpdu_length_cbw40().bits(),
                minimum_mpdu_length_cbw80: queue.minimum_mpdu_length_cbw80().bits(),
                software_rts: queue.software_rts().bit(),
                software_cts: queue.software_cts().bit(),
            }
        });
        let mu_edca_timers = core::array::from_fn(|index| {
            let timer = timers.timer(index).read();
            MacHeMuEdcaTimerSnapshot {
                timer_8tu: timer.timer_8tu().bits(),
                enabled: timer.enable().bit(),
                reset: timer.reset().bit(),
                current_count_8tu: timer.current_count_8tu().bits(),
                reached: timer.reached().bit(),
                aifs: timer.aifs().bits(),
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
