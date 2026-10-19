//! Reviewed unobserved production lines, each with its reason.
use crate::observation::Decision;

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
        reason: "ordinary status-four transmit-error classification (dispatcher argument zero), \
            compared through the stop goal: \
            queues 0/4 and details 0..6/0xc0 each complete only at the independently selected \
            CTS-timeout, ACK-timeout, collision or security-key leaf. A wrong leaf fails the \
            dispatch goal even though dependence on compared register effects does not record \
            that control-flow observation. The context anchor covers only tx_error's dispatch, \
            not rts_error's identical match line or unexecuted detail/selector combinations",
        places: &[
            (
                "driver/ieee80211/mac/src/tx/completion.rs",
                "4 => tx_error(raw.detail),",
            ),
            (
                "driver/ieee80211/mac/src/tx/completion.rs",
                "1 | 3..=5 => Ok(Observed(TxStatus::Collision)),",
            ),
            (
                "driver/ieee80211/mac/src/tx/completion.rs",
                "const fn tx_error(detail: u8) -> Result<CompletionObservation, CompletionFault> {\n\
                use CompletionObservation::Observed;\n\
                match detail {",
            ),
        ],
    },
    Decision {
        reason: "claim of the validation-only radio owner capability in the isolated probe \
            image, which carries no data the leaf reads; the leaf's register effects and return \
            compare",
        places: &[("hal/src/validation.rs", "owner()")],
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
        reason: "execution statistics of the RX-gain and TX-DC executors; they \
            report effort to production diagnostics and have no vendor counterpart",
        places: &[
            (
                "phy/src/rx/gain_calibration.rs",
                "stats.minimum_operations += completion.operations();",
            ),
            (
                "phy/src/rx/gain_calibration.rs",
                "stats.settle_1us += 2 * u32::from(completion.estimators());",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.outer_operations += 1;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.minimum_searches += stats.minimum_searches;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.minimum_operations += stats.minimum_operations;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.settle_1us += stats.settle_1us;",
            ),
        ],
    },
    Decision {
        reason: "failure payload: the measurement, observation count or force-test \
            transaction a timed-out step reports; every compared case completes",
        places: &[
            ("phy/src/tx/dc_power_detector.rs", "let measurement = self"),
            (
                "phy/src/tx/dc_power_detector.rs",
                ".wrapping_add(self.total_measurements);",
            ),
            ("phy/src/tx/dc_power_detector.rs", "for transaction in ["),
            (
                "phy/src/tx/dc_power_detector.rs",
                "PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),",
            ),
        ],
    },
    Decision {
        reason: "spills and reloads of capability references (register access, radio \
            channel, platform, observer) that the probes bind to zero-sized or no-op \
            implementations; the register effects themselves are compared",
        places: &[
            (
                "phy/src/target_port.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            ("phy/src/target_port.rs", "observer: &mut O,"),
            ("phy/src/target_port.rs", "platform: &mut P,"),
            ("phy/src/target_port.rs", "&'port mut self,"),
            ("phy/src/target_port.rs", "&mut self.observer,"),
            ("phy/src/target_port.rs", "self.platform,"),
            ("phy/src/target_port.rs", "self.registers,"),
            (
                "phy/src/target_port/rfpll.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "phy/src/target_port/temperature.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            ("phy/src/validation.rs", "observer: &mut O,"),
            (
                "phy/src/validation.rs",
                "crate::target_port::select_phy_channel_with_hal::<D, _, _>(",
            ),
        ],
    },
    Decision {
        reason: "async future bookkeeping attributed to signatures and closing braces: \
            state-discriminant and local stores into the future frame, register restores, and \
            the ready-flag take of a completed delay future at its `.await`; the probes poll \
            each future to completion without suspension, so no resume reads them",
        places: &[("phy/src/target_port.rs", "}")],
    },
    Decision {
        reason: "probe snapshot artifact: the tracking probes read the RX-gain memory \
            parameters to export their DC banks and drop `parameter_002`, which the RX-gain \
            scenario compares where calibration uses it",
        places: &[(
            "phy/src/state.rs",
            "parameter_002: self.config.pbus_rx_path,",
        )],
    },
    Decision {
        reason: "measurement identity of the event-driven RX-DC host model: the ROM search \
            has no corresponding field and the direct target transaction never reads it",
        places: &[("phy/src/rx/gain_calibration.rs", "policy.iteration,")],
    },
    Decision {
        reason: "state stores the parent probe seeds and the optimized probe forwards in a \
            register to the tracking policy it builds next; each flag's decision (the \
            Bluetooth/802.15.4 TX-power update, the relaxed Wi-Fi TX-power threshold) is \
            observed under both flag values",
        places: &[("phy/src/state.rs", "self.bluetooth.power_tracking = value;")],
    },
    Decision {
        reason: "client ownership bits of the validation-only pending parent fixture; the \
            compared parent transition never reads them",
        places: &[
            (
                "phy/src/state/client.rs",
                "owner.bits = if request.wifi() { WIFI_BIT } else { 0 }",
            ),
            (
                "phy/src/state/client.rs",
                "| if request.bluetooth_ieee802154() {",
            ),
            (
                "phy/src/state/client.rs",
                "owner.tracker_model_armed = owner.bits != 0;",
            ),
        ],
    },
    Decision {
        reason: "initial fields of freshly built tracking children that are written before \
            any read: the calibration child reads each `None` outcome field only after the \
            branch that sets it, and the recalibrated RX-gain child overwrites its initial \
            table results before reading them",
        places: &[("phy/src/target_port.rs", "transition")],
    },
    Decision {
        reason: "the grant-protect port lent to the PHY maintenance ports: the PHY-archive \
            comparison lends the zero-sized `WeakPhyGrantProtect`, whose hooks have no effect \
            like the archive's weak `phy_acquire_grant_protect` and \
            `phy_release_grant_protect`, so passing it carries no data",
        places: &[("phy/src/target_port.rs", "&mut *self.grant,")],
    },
    Decision {
        reason: "temperature acquisition provenance, a production scheduling record with no \
            vendor counterpart; the temperature and sensor index are compared",
        places: &[(
            "phy/src/tracking/temperature.rs",
            "Acquisition::Undated => Self {",
        )],
    },
    Decision {
        reason: "RFPLL correction report: the search and frequency-memory outcome reach only \
            the observer and the maintenance result, the counterpart of the vendor's optional \
            diagnostic `phy_printf`; state keeps only whether a correction happened, and the \
            capacitor and frequency-memory writes are compared",
        places: &[
            ("phy/src/tracking/rfpll/thermal.rs", "self.request"),
            ("phy/src/tracking/parameters.rs", "self.child.request()"),
            ("phy/src/target_port.rs", "let request = child.request();"),
        ],
    },
    Decision {
        reason: "initial correction of the event-driven RX-DC host model; the target \
            runs the calibration as one direct transaction starting from the request",
        places: &[(
            "phy/src/rx/gain_calibration.rs",
            "current: request.initial,",
        )],
    },
    Decision {
        reason: "phase-timer duration and phase notifications of a schedule step, and the \
            idle last phase that re-arms nothing: the coex scenario compares them outside the \
            relation, requiring the production report to equal the vendor's `timer_arm_us` \
            microseconds and the phase callbacks it called",
        places: &[
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "self.wifi != 0",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "self.bluetooth[0] != 0 || self.bluetooth[1] != 0",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "return Err(CoexScheduleIdle::LastPhase);",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "u32::from(scheme.period)",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                ".wrapping_mul(self.interval)",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "notify_wifi: phase.notifies_wifi(),",
            ),
            (
                "crates/hardware/espressif/coex/src/schedule.rs",
                "notify_bluetooth: phase.notifies_bluetooth(),",
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
        reason: "validation-only construction of the isolated arbiter lease and Wi-Fi clock \
            proof in the probe image: they carry no data the compared leaf reads, and its \
            register effects and return compare",
        places: &[
            ("hal/src/shared_radio.rs", "*self.held.get_mut() = true;"),
            ("hal/src/ieee80211/client.rs", "Self {"),
        ],
    },
    Decision {
        reason: "relaxed power-tracking flag stored in the validation state fixture: the \
            compiled probe forwards the same input straight into the tracking policy, whose \
            threshold selection is observed",
        places: &[(
            "phy/src/state.rs",
            "state.wifi.tx_power_tracking_slow = relaxed_threshold.into();",
        )],
    },
    Decision {
        reason: "stop-tone branch of the out-of-line calibration tone: both of its callers, \
            the two `phy_txdc_cal_pwdet_init` instances, pass `enabled = true`, so the branch \
            is always taken to its post-dominating return and no executed step depends on it; \
            the disabled arm is compared at the inlined sites that pass `false`",
        places: &[("pac/src/phy/baseband.rs", "if !enabled {")],
    },
    Decision {
        reason: "constructor default of the filter-DCAP parameters in a fresh PHY state: \
            nothing a compared effect or final state depends on reads it, since the \
            calibration that uses the parameters and the retained-state rebuild write them \
            first",
        places: &[("phy/src/state.rs", "filter_dcap: [0; 5],")],
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
