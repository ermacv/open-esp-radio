//! Reviewed unobserved production lines, each with its reason.
use crate::esp32s31::observation::Decision;

/// Reviewed unobserved lines.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "the CCMP association boundary orders the interface's SPP mode before \
            subsequent key publication with one additional device fence; the hal_crypto_enable \
            contract compares every register read and write for both interfaces, both SPP \
            modes and every fill, and requires exactly one production fence",
        places: &[(
            "pac/src/wifi/mac/crypto.rs",
            "matches!(mode, MacSppMode::Authenticated),\n\
            );\n\
            device_fence();",
        )],
    },
    Decision {
        reason: "device-ordering fences the MAC and power event clears, the ordinary \
            transmit publication and the Bluetooth leaves declared `ordered` (baseband v2 \
            initialization, BLE PHY register initialization, NRT interrupt acknowledge, \
            memory-list pointers, scheduler list heads) add around their register edge are \
            Rust-side additions, not vendor edges: each reviewed contract requires exactly \
            that many, counted but not paired with a vendor effect",
        places: &[
            ("pac/src/wifi/mac/interrupt.rs", "device_fence();"),
            ("pac/src/wifi/mac/tx.rs", "device_fence();"),
            (
                "pac/src/bluetooth/baseband.rs",
                "port.order_device_accesses();",
            ),
            ("pac/src/bluetooth/baseband.rs", "device_fence();"),
            ("pac/src/bluetooth/interrupt.rs", "device_fence();"),
            ("pac/src/bluetooth/memory_lists.rs", "device_fence();"),
            ("pac/src/bluetooth/phy.rs", "device_fence();"),
            ("pac/src/bluetooth/scheduler.rs", "device_fence();"),
            (
                "pac/src/bluetooth/scheduler/insertion.rs",
                "control.order_after_publication();",
            ),
        ],
    },
    Decision {
        reason: "`CoexCore` bookkeeping of a requested or released timer, its active request \
            and its uncertain bit: vendor `coex_core_request` and `coex_core_release` keep no \
            such state, and the probe's fresh core is discarded after the compared timer \
            program or disable, whose register effects and return compare",
        places: &[
            (
                "crates/hardware/espressif/coex/src/core.rs",
                "self.active[usize::from(index.value())] = Some(CoexRequest { client, request });",
            ),
            (
                "crates/hardware/espressif/coex/src/core.rs",
                "self.uncertain_timers |= timer_bit(index);",
            ),
            (
                "crates/hardware/espressif/coex/src/core.rs",
                "self.active[usize::from(index.value())] = None;",
            ),
            (
                "crates/hardware/espressif/coex/src/core.rs",
                "self.uncertain_timers &= !timer_bit(index);",
            ),
        ],
    },
    Decision {
        reason: "end of the low-power clock deselect wrapper, where the validation lease \
            drops: its release is software ownership whose release fence the leaf counts; \
            the deselect register writes are compared by the leaf",
        places: &[("hal/src/bluetooth/validation.rs", "}")],
    },
    Decision {
        reason: "construction of the BLE PHY fixture's shared-radio register owner: no \
            register effect, and the fixture holds it to its end, so only final state \
            depends on it; the compared transaction is `initialize_ble_phy_registers`",
        places: &[(
            "pac/src/validation.rs",
            "let mut shared = shared_radio_registers();",
        )],
    },
    Decision {
        reason: "execution-modify diagnostic selections, which the effect contract leaves \
            unpaired because production reselects after every Pending: the multiplexed \
            diagnostic model checks each of them, failing an undeclared selector and every \
            sample that does not read exactly the runs of the selector in force",
        places: &[
            (
                "hal/src/bluetooth/scheduler_execution_modify.rs",
                "if !selected {",
            ),
            (
                "hal/src/bluetooth/scheduler_execution_modify.rs",
                "hw.select_progress();",
            ),
            (
                "hal/src/bluetooth/scheduler_execution_modify.rs",
                "hw.select_settle();",
            ),
            (
                "hal/src/bluetooth/scheduler_execution_modify.rs",
                ".select_scheduler_execution_modify_settle(self.interrupts);",
            ),
            (
                "pac/src/bluetooth/scheduler/insertion.rs",
                "super::runtime::select_execution_modify_settle(",
            ),
            (
                "pac/src/bluetooth/scheduler/runtime.rs",
                "HardwareDiagnosticControl { registers }.select(DiagnosticSelector::ExecutionModifySettle);",
            ),
        ],
    },
];
