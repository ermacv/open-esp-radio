// Emit only after the complete calibration has released its hardware controls.
pub(super) async fn log(diagnostics: Option<oer_esp32s31_phy::RfCalibrationDiagnostics>) {
    let Some(diagnostics) = diagnostics else {
        crate::console::runtime_log_reliably(format_args!(
            "hil-phy: RF calibration observations unavailable"
        ))
        .await;
        return;
    };
    crate::console::runtime_log_reliably(format_args!(
        "hil-phy: charge_pump_locked={}",
        diagnostics.charge_pump_locked
    ))
    .await;
    if let Some(frequency) = diagnostics.frequency {
        for (name, point) in [
            ("nominal", frequency.nominal),
            ("low", frequency.low),
            ("high", frequency.high),
        ] {
            crate::console::runtime_log_reliably(format_args!(
                "hil-phy: point={} lock={} cap_initial={} cap_final={} accepted={}",
                name,
                point.lock_observed,
                point.initial_cap,
                point.final_cap,
                point.accepted_cap_samples,
            ))
            .await;
        }
        crate::console::runtime_log_reliably(format_args!(
            "hil-phy: frequency_table_entries={}",
            frequency.table_entries
        ))
        .await;
    }
}
