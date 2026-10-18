//! Host control for bounded connected-STA lifecycle qualification.

use crate::link::WifiCapture as _;
use oer_hil_workload::context::Context;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use crate::Result;
use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    phy::StartupArtifactDisposition, phy::StartupArtifactStatus, wifi::StationAttemptFailureReason,
    wifi::StationEpochEvidence, wifi::StationLifecycleEvent,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CYCLES: u8 = 1;
const MAX_CYCLES: u8 = 8;
const DEFAULT_BOOTS: u8 = 1;
const MAX_BOOTS: u8 = 100;
const MAX_INITIAL_HOLD: Duration = Duration::from_secs(30);
pub struct Config {
    pub timeout: Duration,
    pub cycles: u8,
    pub boots: u8,
    pub initial_hold: Duration,
}

pub fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let options = options.validate()?;
    if options.boots > 1 && context.lab.dut.startup_artifact.is_none() {
        return Err(
            "multi-boot station lifecycle qualification requires a configured startup artifact"
                .into(),
        );
    }
    fs::create_dir_all(output)?;
    for boot in 1..=options.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let capture = context.capture(&boot_output)?;
        let result = qualify(
            &capture,
            context,
            options.timeout,
            options.cycles,
            options.initial_hold,
            boot,
        );
        let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
        let result = capture.finish_with(result);
        let boot_log = boot_output.join("uart.log");
        result.map_err(|error| {
            oer_hil_link::error::context(
                format!(
                    "station cold boot {boot}/{} failed; UART evidence: {}",
                    options.boots,
                    boot_log.display(),
                ),
                error,
            )
        })?;
        if let Some(result) = beacon_loss {
            result?;
        }
        eprintln!("station_cold_boot={boot}/{} status=PASS", options.boots);
    }
    eprintln!("station_reconnect=PASS boots={}", options.boots);
    eprintln!("uart_logs={}", output.display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
    cycles: u8,
    initial_hold: Duration,
    boot: u8,
) -> Result<()> {
    // Arm the unsolicited-event cursor before provisioning can make the
    // station connect. With a correctly clocked target, scan/join may finish
    // before `prepare_station` returns its correlated acknowledgement; taking
    // the cursor afterwards skips that already-published Connected edge and
    // waits forever for a second one.
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let (capabilities, startup_artifact_status) =
        capture.prepare_station_with_startup_artifact_status(context.target(), timeout)?;
    validate_startup_artifact_replay(boot, startup_artifact_status)?;
    if !capabilities.has::<oer_hil_protocol::wifi::StationEpochControl>() {
        return Err("firmware does not advertise station epoch control".into());
    }
    // Network provisioning is acknowledged before the radio task finishes
    // scan/join/WPA2.  A lifecycle command is valid only after the unsolicited
    // connected edge has published the Station owner at the public boundary.
    let mut generation = capture.wait_for_connected_station(timeout)?;
    wait_for_connected_generation(capture, &mut lifecycle_cursor, generation, timeout)?;
    qualify_initial_hold(capture, &mut lifecycle_cursor, generation, initial_hold)?;
    report_stack(capture, timeout, "initial-connected")?;
    for cycle in 1..=cycles {
        generation = qualify_cycle(capture, timeout, cycle, generation)?;
        report_stack(capture, timeout, "reconnect-complete")?;
        eprintln!("station_reconnect_cycle={cycle}/{cycles} status=PASS");
    }
    Ok(())
}

fn validate_startup_artifact_replay(boot: u8, status: Option<StartupArtifactStatus>) -> Result<()> {
    if boot == 1 {
        return Ok(());
    }
    let status = status.ok_or_else(|| {
        format!("station cold boot {boot} did not report startup artifact restoration")
    })?;
    match status.disposition {
        StartupArtifactDisposition::Restored => Ok(()),
        disposition => Err(format!(
            "station cold boot {boot} did not restore the artifact persisted by the preceding boot: {disposition:?}"
        )
        .into()),
    }
}

fn wait_for_connected_generation(
    capture: &SerialCapture,
    cursor: &mut usize,
    expected_generation: u32,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = capture.wait_station_lifecycle_event(cursor, remaining)?;
        if matches!(event, StationLifecycleEvent::Connected { generation, .. } if generation == expected_generation)
        {
            return Ok(());
        }
        if matches!(event, StationLifecycleEvent::Disconnected { .. }) {
            return Err(format!(
                "station disconnected before connected generation {expected_generation} could be held"
            )
            .into());
        }
        if matches!(event, StationLifecycleEvent::Faulted { .. }) {
            return Err(format!(
                "station faulted before connected generation {expected_generation}: {event:?}"
            )
            .into());
        }
    }
}

fn qualify_initial_hold(
    capture: &SerialCapture,
    cursor: &mut usize,
    generation: u32,
    duration: Duration,
) -> Result<()> {
    if duration.is_zero() {
        return Ok(());
    }
    if let Some(event) = capture.wait_station_lifecycle_event_optional(cursor, duration)? {
        return Err(format!(
            "station generation {generation} changed during the initial {}-second hold: {event:?}",
            duration.as_secs()
        )
        .into());
    }
    eprintln!(
        "station_initial_hold_generation={generation} seconds={} status=PASS",
        duration.as_secs()
    );
    Ok(())
}

fn report_stack(capture: &SerialCapture, timeout: Duration, stage: &str) -> Result<()> {
    let usage = capture.query_stack_usage(timeout)?;
    eprintln!(
        "stack_stage={stage} cpu0_free={}/{} cpu0_required={} cpu1_free={}/{} cpu1_required={}",
        usage.cpu0.free_bytes,
        usage.cpu0.capacity_bytes,
        usage.cpu0.minimum_free_bytes,
        usage.cpu1.free_bytes,
        usage.cpu1.capacity_bytes,
        usage.cpu1.minimum_free_bytes,
    );
    Ok(())
}

#[derive(Default)]
struct CycleProgress {
    disconnected: bool,
    connected: bool,
    owners_complete: bool,
}

impl CycleProgress {
    fn observe_lifecycle(
        &mut self,
        event: StationLifecycleEvent,
        cycle: u8,
        generation: u32,
    ) -> Result<()> {
        match event {
            StationLifecycleEvent::Disconnected {
                generation: observed,
                reason: oer_hil_protocol::wifi::StationDisconnectReason::ReconnectRequested,
            } if observed == generation && !self.disconnected => {
                self.disconnected = true;
                Ok(())
            }
            StationLifecycleEvent::Connected {
                generation: observed,
                ..
            } if self.disconnected && observed == generation.wrapping_add(1) => {
                self.connected = true;
                Ok(())
            }
            event @ (StationLifecycleEvent::AttemptFailed { .. }
            | StationLifecycleEvent::RetryExhausted { .. }) => {
                validate_cycle_event(event, cycle)
            }
            event => Err(format!(
                    "station reconnect cycle {cycle} published an unexpected lifecycle edge: \
                     expected generation {generation} disconnect then generation {} connect, got {event:?}",
                    generation.wrapping_add(1),
                )
                .into()),
        }
    }

    const fn complete(&self) -> bool {
        self.disconnected && self.connected && self.owners_complete
    }
}

fn qualify_cycle(
    capture: &SerialCapture,
    timeout: Duration,
    cycle: u8,
    generation: u32,
) -> Result<u32> {
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let handle = capture.request_station_epoch_cycle()?;
    let mut progress = CycleProgress::default();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        oer_process::check_cancelled()?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(20));
        if let Some(event) =
            capture.wait_station_lifecycle_event_optional(&mut lifecycle_cursor, wait)?
        {
            progress.observe_lifecycle(event, cycle, generation)?;
        }
        if !progress.owners_complete {
            progress.owners_complete = validate_cycle_completion(
                capture.observed_station_epoch_completion(handle),
                cycle,
            )?;
        }
        if progress.complete() {
            return Ok(generation.wrapping_add(1));
        }
    }
    Err(format!(
        "target did not complete station reconnect cycle {cycle} within {} seconds \
         (disconnected={}, connected={}, owners_complete={})",
        timeout.as_secs(),
        progress.disconnected,
        progress.connected,
        progress.owners_complete,
    )
    .into())
}

fn validate_cycle_event(event: StationLifecycleEvent, cycle: u8) -> Result<()> {
    match event {
        StationLifecycleEvent::RetryExhausted { .. } => {
            Err(format!("station lifecycle exhausted retries in cycle {cycle}: {event:?}").into())
        }
        StationLifecycleEvent::AttemptFailed {
            reason:
                StationAttemptFailureReason::Hardware | StationAttemptFailureReason::ContractViolation,
            ..
        } => Err(
            format!("station lifecycle reported fatal failure in cycle {cycle}: {event:?}").into(),
        ),
        _ => Ok(()),
    }
}

fn validate_cycle_completion(completion: Option<StationEpochEvidence>, cycle: u8) -> Result<bool> {
    let Some(completion) = completion else {
        return Ok(false);
    };
    if !completion.is_complete() {
        return Err(format!(
            "station epoch cycle {cycle} completed with incomplete typed evidence: {completion:?}"
        )
        .into());
    }
    Ok(true)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            cycles: DEFAULT_CYCLES,
            boots: DEFAULT_BOOTS,
            initial_hold: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests;

impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(10)..=Duration::from_secs(300)).contains(&self.timeout) {
            return Err("reconnect timeout must be in 10..=300 seconds".into());
        }
        if !(1..=MAX_CYCLES).contains(&self.cycles) {
            return Err("reconnect cycles must be in 1..=8".into());
        }
        if !(1..=MAX_BOOTS).contains(&self.boots) {
            return Err("reconnect boots must be in 1..=100".into());
        }
        if self.initial_hold > MAX_INITIAL_HOLD {
            return Err("initial hold must be at most 30 seconds".into());
        }
        Ok(self)
    }
}
