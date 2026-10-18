//! Reviewed unprojected vendor state, each with its reason.
use crate::state::{Decision, Place};

/// The claim a place belongs to: vendor root and production entry.
type Claim = (&'static str, &'static str);

const fn place(claim: Claim, symbol: &'static str, start: u32, end: u32) -> Place {
    Place {
        root: claim.0,
        production: claim.1,
        symbol,
        start,
        end,
    }
}

const EXTERNAL_MODE: Claim = (
    "hal_set_extern_pti_mode",
    "open_coex_external_trace_pti_mode",
);
const EXTERNAL_PARAMS: Claim = (
    "ic_set_extern_coex_params",
    "open_coex_external_trace_params",
);
const PARENT: Claim = ("phy_param_track_tot", "open_phy_tracking_trace_parent");
const COMBINED: Claim = ("phy_cal_param_track", "open_phy_calibration_trace_combined");
const RX_GAIN: Claim = (
    "phy_set_rx_gain_table",
    "open_phy_calibration_trace_rx_gain",
);
const CHANNEL: Claim = ("phy_chip_set_chan", "open_phy_channel_trace_state");
const BLUETOOTH_GAIN: Claim = ("phy_bt_set_tx_gain_new", "open_phy_bluetooth_trace_tx_gain");
const RFPLL_MAINTAIN: Claim = ("phy_rfpll_cap_track_new", "open_phy_rfpll_trace_maintain");
const RFPLL_THERMAL: Claim = ("phy_rfpll_cap_track_new", "open_phy_rfpll_trace_track");
const AP_TSF_START: Claim = (
    "hal_mac_tsf_reset",
    "open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset",
);
const RX_POLICY: Claim = (
    "wifi_set_rx_policy",
    "open_wifi_sta_ap_trace_wifi_set_rx_policy",
);
const RX_POLICY_NONE: Claim = (
    "wifi_set_rx_policy",
    "open_wifi_sta_ap_trace_disable_all_role_receive",
);

const LOW_POWER_CLOCK_SELECT: Claim = (
    "modem_clock_select_lp_clock_source",
    "open_bluetooth_trace_select_low_power_clock",
);
const LOW_POWER_CLOCK_DESELECT: Claim = (
    "modem_clock_deselect_lp_clock_source",
    "open_bluetooth_trace_deselect_low_power_clock",
);
const WIFI_LOW_POWER_CLOCK_SELECT: Claim = (
    "modem_clock_select_lp_clock_source",
    "open_modem_clock_trace_select_wifi_low_power_clock",
);
/// `modem_clock_context_t` of ESP-IDF `esp_hw_support/modem`: the `dev` and
/// `icg_config` pointers, and the Wi-Fi and Bluetooth entries of `lpclk_src`.
const MODEM_CLOCK_CONTEXT: &str = "modem_clock_context.7";
const MODEM_CLOCK_POINTERS: (u32, u32) = (12, 20);
const MODEM_CLOCK_WIFI_SOURCE: (u32, u32) = (20, 24);
/// The static reference count of `modem_clock_hal_enable_wifipwr_clock`.
const WIFI_POWER_CLOCK_REFERENCES: &str = "ref.1";
const WIFI_POWER_CLOCK_REFERENCE_COUNT: (u32, u32) = (0, 4);
const MODEM_CLOCK_BLUETOOTH_SOURCE: (u32, u32) = (24, 28);
/// The flags of the I2C-master entry of the ESP32-S31 modem device context.
const MODEM_CLOCK_DEVICES: &str = "dev.4";
const MODEM_CLOCK_I2C_MASTER_FLAGS: (u32, u32) = (18, 20);

const RETRY_CTS: Claim = ("lmacProcessCtsTimeout", "open_libpp_tx_retry_trace_step");
const RETRY_COLLISION: Claim = ("lmacProcessCollision", "open_libpp_tx_retry_trace_step");
const RETRY_ACK: Claim = ("lmacProcessAckTimeout", "open_libpp_tx_retry_trace_step");
/// The retry sequences' queue contexts, named by the nearest symbol below
/// them, and the two per-queue bytes they leave unprojected.
const RETRY_QUEUES: &str = "0x2f850000";
const RETRY_QUEUES_OFFSET: u32 = 0x3fff_2000 - 0x2f85_0000;
const RETRY_QUEUE_BYTES: u32 = 0x38;
const RETRY_SHORT_COUNT: u32 = 0x0b;
const RETRY_STATE: u32 = 0x12;

const fn retry_queue(claim: Claim, field: u32, length: u32, queue: u32) -> Place {
    let start = RETRY_QUEUES_OFFSET + queue * RETRY_QUEUE_BYTES + field;
    place(claim, RETRY_QUEUES, start, start + length)
}

/// Reviewed unprojected vendor state.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "`lmac.o` per-queue short and long retry counts (queue context 0x0b and \
            0x0c) that `lmacProcess{Short,Long}RetryFail` raise and compare with the retry \
            limits to reset the contention window: production has no queue-level counts and \
            resets the window when the MPDU's own count reaches the limit. The compared \
            sequences send one MSDU per queue without interleaving, where the counts are equal \
            and the compared contention exponent follows; interleaved MSDUs are not covered",
        places: &[
            retry_queue(RETRY_CTS, RETRY_SHORT_COUNT, 1, 0),
            retry_queue(RETRY_CTS, RETRY_SHORT_COUNT, 1, 1),
            retry_queue(RETRY_CTS, RETRY_SHORT_COUNT, 1, 2),
            retry_queue(RETRY_CTS, RETRY_SHORT_COUNT, 1, 3),
            retry_queue(RETRY_COLLISION, RETRY_SHORT_COUNT, 2, 0),
            retry_queue(RETRY_COLLISION, RETRY_SHORT_COUNT, 2, 1),
            retry_queue(RETRY_COLLISION, RETRY_SHORT_COUNT, 2, 2),
            retry_queue(RETRY_COLLISION, RETRY_SHORT_COUNT, 2, 3),
            retry_queue(RETRY_ACK, RETRY_SHORT_COUNT, 1, 0),
            retry_queue(RETRY_ACK, RETRY_SHORT_COUNT, 1, 1),
            retry_queue(RETRY_ACK, RETRY_SHORT_COUNT, 1, 2),
            retry_queue(RETRY_ACK, RETRY_SHORT_COUNT, 1, 3),
        ],
    },
    Decision {
        reason: "`lmac.o` per-queue exchange state (queue context 0x12) that the vendor \
            transmit path sets and the retry-limit branch moves to its end state before the \
            modeled exchange end: production keeps the transmission in its ordinary TX owner, \
            whose compared retry decision ends it",
        places: &[
            retry_queue(RETRY_CTS, RETRY_STATE, 1, 0),
            retry_queue(RETRY_CTS, RETRY_STATE, 1, 1),
            retry_queue(RETRY_CTS, RETRY_STATE, 1, 2),
            retry_queue(RETRY_CTS, RETRY_STATE, 1, 3),
            retry_queue(RETRY_COLLISION, RETRY_STATE, 1, 0),
            retry_queue(RETRY_COLLISION, RETRY_STATE, 1, 1),
            retry_queue(RETRY_COLLISION, RETRY_STATE, 1, 2),
            retry_queue(RETRY_COLLISION, RETRY_STATE, 1, 3),
            retry_queue(RETRY_ACK, RETRY_STATE, 1, 0),
            retry_queue(RETRY_ACK, RETRY_STATE, 1, 1),
            retry_queue(RETRY_ACK, RETRY_STATE, 1, 2),
            retry_queue(RETRY_ACK, RETRY_STATE, 1, 3),
        ],
    },
    Decision {
        reason: "`ieee80211_supplicant.o` current-policy byte `g_ic+0x2cc`: only the \
            remain-on-channel policy 13 saves it into `g_offchan_ctx` and `roc_op_end` tests it \
            for that policy; production has no remain-on-channel owner, and its role owners \
            hold the applied receive configuration as the typed `MacRoleReceivePolicy` they pass",
        places: &[
            place(RX_POLICY, "g_ic", 0x2cc, 0x2cd),
            place(RX_POLICY_NONE, "g_ic", 0x2cc, 0x2cd),
        ],
    },
    Decision {
        reason: "`if_hwctrl.o` interface-one address shadow in `if_ctrl` that `ic_set_mac` \
            copies after programming the address registers, for `ic_get_addr` callers; \
            production callers own the access-point address they pass to \
            `configure_role_receive_policy`, and the register writes are compared",
        places: &[place(RX_POLICY, "if_ctrl", 10, 16)],
    },
    Decision {
        reason: "`wdev.o` beacon-schedule cursor `BcnSendTick` that a fresh AP TSF epoch clears \
            for `wDev_Get_Next_TBTT`: production keeps the TBTT cursor in the AP engine's beacon \
            state, which each started engine creates empty, not in the MAC TSF leaf",
        places: &[place(AP_TSF_START, "BcnSendTick", 0, 4)],
    },
    Decision {
        reason: "`phy_track.o` static `s_track_result`, named by its section: a debug \
            copy of the current, power, common and transmit reference temperatures, the RFPLL \
            reference and the progress word, all compared as `phy_param` fields of the parent \
            claim; only the exported `phy_debug_get_track_result` reads it, and no vendor \
            library or ROM function calls that",
        places: &[place(PARENT, ".bss.s_track_result", 0, 14)],
    },
    Decision {
        reason: "`phy_force_txrx_off_new` nesting count: every force/release pair of the \
            parent returns it to its entry value. Production encodes the count each pair \
            observes as its static `PhyForceTxRxDepth`, whose selected force and release \
            sequences are compared as register effects",
        places: &[
            place(PARENT, "phy_param", 0x1ea, 0x1ec),
            place(COMBINED, "phy_param", 0x1ea, 0x1ec),
            place(CHANNEL, "phy_param", 0x1ea, 0x1ec),
            place(BLUETOOTH_GAIN, "phy_param", 0x1ea, 0x1ec),
        ],
    },
    Decision {
        reason: "readiness activity edge count of one DC estimate: `phy_iq_est_enable` clears \
            it unconditionally before counting and only `phy_rxdc_est_min` of the same estimate \
            reads it, so no later estimate or caller observes the stored count; the admission \
            it decides is compared through the published DC codes",
        places: &[
            place(RX_GAIN, "phy_param", 0x1ac, 0x1ae),
            place(COMBINED, "phy_param", 0x1ac, 0x1ae),
            place(PARENT, "phy_param", 0x1ac, 0x1ae),
        ],
    },
    Decision {
        reason: "upper halfword of the calibration status word, rewritten unchanged: every \
            vendor status update is a word read-modify-write whose flag bits (0x8, 0x20, 0x80, \
            0x200 and the 0x221 clear) lie in the compared lower halfword",
        places: &[
            place(RX_GAIN, "phy_param", 0xa6, 0xa8),
            place(COMBINED, "phy_param", 0xa6, 0xa8),
            place(PARENT, "phy_param", 0xa6, 0xa8),
        ],
    },
    Decision {
        reason: "RX-gain completion flags (0x80 and 0x200 of the status word) and the common \
            reference temperature `phy_set_rx_gain_table` copies from the current temperature \
            after generating tables: production commits them in its state owner \
            (`apply_rx_gain_init_outcome` and the calibration-tracking commit), which the \
            RX-gain child probe does not run; the combined and parent claims compare both after \
            the same child",
        places: &[
            place(RX_GAIN, "phy_param", 0xa4, 0xa6),
            place(RX_GAIN, "phy_param", 0x190, 0x192),
        ],
    },
    Decision {
        reason: "wide-bandwidth flag `phy_chip_set_chan` derives as `bandwidth != 0` from the \
            compared bandwidth byte; its only reader is ROM `phy_get_pwr_index`, which only \
            `librftest.a` calls, and production has no RF-test power-index path",
        places: &[
            place(CHANNEL, "phy_param", 0x11e, 0x11f),
            place(COMBINED, "phy_param", 0x11e, 0x11f),
            place(PARENT, "phy_param", 0x11e, 0x11f),
        ],
    },
    Decision {
        reason: "reference temperature and RFPLL progress bit the thermal child commits after \
            the correction; the maintenance claim compares the correction's frequency-control \
            effects, and the thermal claim of the same root compares both commits",
        places: &[
            place(RFPLL_MAINTAIN, "phy_param", 0x130, 0x132),
            place(RFPLL_MAINTAIN, "phy_param", 0x1e6, 0x1e8),
        ],
    },
    Decision {
        reason: "RFPLL tracking reentrancy guard: `phy_rfpll_cap_track_new` sets it after \
            admission and clears it before returning, so the stored byte is unchanged by every \
            completed call; production serializes the child by ownership instead",
        places: &[
            place(RFPLL_MAINTAIN, "phy_param", 0x194, 0x195),
            place(RFPLL_THERMAL, "phy_param", 0x194, 0x195),
            place(PARENT, "phy_param", 0x194, 0x195),
        ],
    },
    Decision {
        reason: "external coexistence follower flag: the vendor records the work mode in a \
            static byte its priority writes read later; production passes the mode with each \
            operation instead, and the follower priority leaves seed the byte for each mode, \
            so the flag's effect is compared through them",
        places: &[
            place(EXTERNAL_MODE, ".bss.s_external_coex_is_slv_mode", 0, 1),
            place(EXTERNAL_PARAMS, ".bss.s_external_coex_is_slv_mode", 0, 1),
        ],
    },
    Decision {
        reason: "lazy initialization of the ESP-IDF modem clock singleton on its first call \
            (`MODEM_CLOCK_instance` stores the `dev` and `icg_config` pointers, and \
            `modem_clock_device_context` clears the reference-count flag of the I2C master \
            device): production owns the modem clock registers through typed owners \
            constructed at boot and has no lazily built context",
        places: &[
            place(
                LOW_POWER_CLOCK_SELECT,
                MODEM_CLOCK_CONTEXT,
                MODEM_CLOCK_POINTERS.0,
                MODEM_CLOCK_POINTERS.1,
            ),
            place(
                LOW_POWER_CLOCK_SELECT,
                MODEM_CLOCK_DEVICES,
                MODEM_CLOCK_I2C_MASTER_FLAGS.0,
                MODEM_CLOCK_I2C_MASTER_FLAGS.1,
            ),
            place(
                LOW_POWER_CLOCK_DESELECT,
                MODEM_CLOCK_CONTEXT,
                MODEM_CLOCK_POINTERS.0,
                MODEM_CLOCK_POINTERS.1,
            ),
            place(
                LOW_POWER_CLOCK_DESELECT,
                MODEM_CLOCK_DEVICES,
                MODEM_CLOCK_I2C_MASTER_FLAGS.0,
                MODEM_CLOCK_I2C_MASTER_FLAGS.1,
            ),
            place(
                WIFI_LOW_POWER_CLOCK_SELECT,
                MODEM_CLOCK_CONTEXT,
                MODEM_CLOCK_POINTERS.0,
                MODEM_CLOCK_POINTERS.1,
            ),
            place(
                WIFI_LOW_POWER_CLOCK_SELECT,
                MODEM_CLOCK_DEVICES,
                MODEM_CLOCK_I2C_MASTER_FLAGS.0,
                MODEM_CLOCK_I2C_MASTER_FLAGS.1,
            ),
        ],
    },
    Decision {
        reason: "the vendor's record of the Bluetooth low-power clock source, kept for the \
            light-sleep power-domain configuration (`esp_sleep_pd_config`, answered without \
            effect) and the next deselect: production records the selection as the shared \
            radio's `bluetooth_low_power_clock` flag, which refuses a second select, and has \
            no light-sleep domain management; the compared register effects select and \
            deselect the same source",
        places: &[
            place(
                LOW_POWER_CLOCK_SELECT,
                MODEM_CLOCK_CONTEXT,
                MODEM_CLOCK_BLUETOOTH_SOURCE.0,
                MODEM_CLOCK_BLUETOOTH_SOURCE.1,
            ),
            place(
                LOW_POWER_CLOCK_DESELECT,
                MODEM_CLOCK_CONTEXT,
                MODEM_CLOCK_BLUETOOTH_SOURCE.0,
                MODEM_CLOCK_BLUETOOTH_SOURCE.1,
            ),
        ],
    },
    Decision {
        reason: "the vendor's record of the Wi-Fi low-power clock source, which only the \
            light-sleep power-domain configuration reads (`esp_sleep_pd_config` keeps the \
            selected source's domain powered in sleep and releases the previous one; answered \
            without effect): production selects the source once at the first cold radio \
            power-up, never deselects it and has no light-sleep domain management, so it \
            keeps no record (reviewed with the Wi-Fi owner); the compared register effects \
            select the same source",
        places: &[place(
            WIFI_LOW_POWER_CLOCK_SELECT,
            MODEM_CLOCK_CONTEXT,
            MODEM_CLOCK_WIFI_SOURCE.0,
            MODEM_CLOCK_WIFI_SOURCE.1,
        )],
    },
    Decision {
        reason: "the vendor's reference count of the Wi-Fi power clock gate, which \
            `modem_clock_hal_enable_wifipwr_clock` increments on each selection and a disable \
            decrements, gating the clock off at zero: production sets the gate once at the \
            first cold radio power-up and never clears it, so it counts no references \
            (reviewed with the Wi-Fi owner); the compared register effects set the same gate",
        places: &[place(
            WIFI_LOW_POWER_CLOCK_SELECT,
            WIFI_POWER_CLOCK_REFERENCES,
            WIFI_POWER_CLOCK_REFERENCE_COUNT.0,
            WIFI_POWER_CLOCK_REFERENCE_COUNT.1,
        )],
    },
];
