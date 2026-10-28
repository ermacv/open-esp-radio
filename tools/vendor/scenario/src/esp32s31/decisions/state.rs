//! Reviewed unprojected vendor state, each with its reason.
use crate::esp32s31::state::{Decision, Place};

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
/// `modem_clock_context_t` of ESP-IDF `esp_hw_support/modem`: the `dev` and
/// `icg_config` pointers, and the Wi-Fi and Bluetooth entries of `lpclk_src`.
const MODEM_CLOCK_CONTEXT: &str = "modem_clock_context.7";
const MODEM_CLOCK_POINTERS: (u32, u32) = (12, 20);
const MODEM_CLOCK_BLUETOOTH_SOURCE: (u32, u32) = (24, 28);
/// The flags of the I2C-master entry of the ESP32-S31 modem device context.
const MODEM_CLOCK_DEVICES: &str = "dev.4";
const MODEM_CLOCK_I2C_MASTER_FLAGS: (u32, u32) = (18, 20);

/// Reviewed unprojected vendor state.
pub const DECISIONS: &[Decision] = &[
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
];
