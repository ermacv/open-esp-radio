//! Ordered cold-power restoration with a readback boundary after every stage.

/// The first stage that did not restore; later stages have not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerRestoreCheckpoint {
    ModemSyscon,
    ModemSourceClocks,
    ModemRegisterBusClock,
}

/// Opaque chip snapshots never grant register authority to the driver.
pub trait PowerBaselinePort {
    type Baseline: Copy;
    fn restore_syscon(&mut self, baseline: Self::Baseline);
    fn syscon_matches(&self, baseline: Self::Baseline) -> bool;
    fn restore_source(&mut self, baseline: Self::Baseline);
    fn source_matches(&self, baseline: Self::Baseline) -> bool;
    fn restore_bus(&mut self, baseline: Self::Baseline);
    fn bus_matches(&self, baseline: Self::Baseline) -> bool;
}

/// Restore non-monotonic fields. ICG state maps remain installed.
pub fn restore<P: PowerBaselinePort>(
    port: &mut P,
    baseline: P::Baseline,
) -> Result<(), PowerRestoreCheckpoint> {
    port.restore_syscon(baseline);
    if !port.syscon_matches(baseline) {
        return Err(PowerRestoreCheckpoint::ModemSyscon);
    }
    port.restore_source(baseline);
    if !port.source_matches(baseline) {
        return Err(PowerRestoreCheckpoint::ModemSourceClocks);
    }
    port.restore_bus(baseline);
    if !port.bus_matches(baseline) {
        return Err(PowerRestoreCheckpoint::ModemRegisterBusClock);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
