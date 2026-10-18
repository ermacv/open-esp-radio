//! Linux DTM fixture preparation, isolated from ESP firmware execution.

pub mod att;
pub mod att_parameters;
pub mod discovery;
pub use oer_hil_fixture::bluetooth::model;
pub mod secure_gatt;

use model::{Adapter, Check, DtmProfile, DtmVersion};
use oer_hil_protocol::bluetooth::BluetoothPeripheralTermination;
use oer_hil_run_bundle_format::run::fixtures::{self, Helper};
use std::{
    fs,
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn check(root: &Path, adapter: Adapter, dtm_version: DtmVersion) -> crate::Result<()> {
    let _lease = oer_hil_lab::lock::acquire_bluetooth(adapter)?;
    let directory = root.join("target/hil/fixture-checks");
    fs::create_dir_all(&directory)?;
    let output = tempfile::Builder::new()
        .prefix(&format!(
            "{}-bluetooth-",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
        ))
        .tempdir_in(directory)?
        .keep();
    let result = run_profile_in(&output, adapter, dtm_version, DtmProfile::ReceiveTransmit);
    crate::emit_json(
        &serde_json::from_slice::<serde_json::Value>(&fs::read(output.join("result.json"))?)?,
        true,
    )?;
    result.map(|_| ())
}

/// What the privileged helper printed, kept as process output; its typed
/// report becomes the repetition's helper fixture record.
const HELPER_STDOUT: &str = "helper.stdout";

/// The helper's report in `output`: when it printed a valid one, recorded as
/// the typed [`Helper`] fixture record and returned; otherwise `absent()`,
/// and no record is written.
fn helper_record<R>(output: &Path, absent: impl FnOnce() -> R) -> crate::Result<R>
where
    R: serde::Serialize + serde::de::DeserializeOwned,
{
    let Some(report) = fs::read(output.join(HELPER_STDOUT))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<R>(&bytes).ok())
    else {
        return Ok(absent());
    };
    let record = Helper(report);
    fixtures::write(output, &record)?;
    Ok(record.0)
}

pub fn run_in(output: &Path, adapter: Adapter, profile: DtmProfile) -> crate::Result<Check> {
    run_profile_in(output, adapter, DtmVersion::V2, profile)
}

fn run_profile_in(
    output: &Path,
    adapter: Adapter,
    dtm_version: DtmVersion,
    profile: DtmProfile,
) -> crate::Result<Check> {
    fs::create_dir_all(output)?;
    let mut command = oer_process::command("sudo");
    command
        .args([
            "-n",
            "/usr/local/libexec/open-radio-bluetooth",
            "check",
            "--adapter",
            &adapter.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join(HELPER_STDOUT))?))
        .stderr(Stdio::from(fs::File::create(output.join("helper.stderr"))?));
    if dtm_version == DtmVersion::V1 {
        command.args(["--dtm-version", "v1"]);
    }
    command.args(["--profile", profile.argument()]);
    // The helper has its own bounded operations and receives SIGTERM before
    // escalation. Keep enough grace for Reset, re-registration and restoration.
    let result = (|| -> crate::Result<Check> {
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        let status = child.wait_timeout(Some(Duration::from_secs(45)))?;
        checked_report(
            status.success(),
            adapter,
            dtm_version,
            profile,
            &fs::read(output.join(HELPER_STDOUT))?,
        )
        .map_err(|error| format!("{error}; evidence: {}", output.display()).into())
    })();
    let report = helper_record(output, || Check::new(adapter, dtm_version, profile))?;
    let summary = serde_json::json!({
        "schema": 2, "adapter": adapter.to_string(), "output": output,
        "dtm_version": dtm_version,
        "profile": profile,
        "passed": result.is_ok(), "rf_verified": false,
        "error": result.as_ref().err().map(ToString::to_string),
        "helper": report,
    });
    oer_durable::atomic_json(&output.join("result.json"), &summary)?;
    result
}

fn checked_report(
    success: bool,
    adapter: Adapter,
    dtm_version: DtmVersion,
    profile: DtmProfile,
    bytes: &[u8],
) -> crate::Result<Check> {
    let report: Check = serde_json::from_slice(bytes).map_err(|error| {
        format!("Bluetooth helper returned no valid report ({error}); inspect helper.stderr. Install with cargo stand fixture install --provider linux-bluetooth")
    })?;
    if !success || !report.passed(adapter, dtm_version, profile) {
        return Err(format!(
            "Bluetooth check failed, incomplete, or profile mismatch (expected {dtm_version:?} {profile:?}, received {:?} {:?}): {}",
            report.dtm_version,
            report.profile,
            report.errors.join("; ")
        )
        .into());
    }
    Ok(report)
}

pub fn preflight(adapter: Adapter) -> crate::Result<()> {
    att_parameters::require_clean(adapter)?;
    const HELPER: &str = "/usr/local/libexec/open-radio-bluetooth";
    if !Path::new(HELPER).is_file() {
        return Err(
            "install the Bluetooth helper with cargo stand fixture install --provider linux-bluetooth".into(),
        );
    }
    let mut capabilities = oer_process::command(HELPER);
    capabilities.arg("capabilities");
    let capabilities = oer_process::output(&mut capabilities, Some(Duration::from_secs(5)))?;
    require_helper_capabilities(capabilities.status.success(), &capabilities.stdout)?;

    // The policy must grant every DTM profile exactly as the runner invokes it.
    for profile in DtmProfile::ALL {
        let mut command = oer_process::command("sudo");
        command.args([
            "-n",
            "-l",
            HELPER,
            "check",
            "--adapter",
            &adapter.to_string(),
            "--profile",
            profile.argument(),
        ]);
        let output = oer_process::output(&mut command, Some(Duration::from_secs(5)))?;
        if !output.status.success() {
            return Err(
                "install the Bluetooth helper with cargo stand fixture install --provider linux-bluetooth".into(),
            );
        }
    }
    if !Path::new("/sys/class/bluetooth")
        .join(adapter.to_string())
        .exists()
    {
        return Err(format!("Bluetooth adapter {adapter} is absent").into());
    }
    Ok(())
}

fn require_helper_capabilities(success: bool, stdout: &[u8]) -> crate::Result<()> {
    if success && stdout == format!("{}\n", model::HELPER_CAPABILITIES).as_bytes() {
        return Ok(());
    }
    Err("installed Bluetooth helper is incompatible; rerun cargo stand fixture install --provider linux-bluetooth".into())
}

/// Prove password-free admission without acquiring an adapter: Clap rejects the invalid peer
/// before launcher adoption or any RF operation. `sudo -l` alone permits password-required rules.
pub fn preflight_security_failure(adapter: Adapter) -> crate::Result<()> {
    preflight(adapter)?;
    for (failure, read_version) in [
        ("missing-key", false),
        ("wrong-key", false),
        ("missing-refresh-key", false),
        ("active-data-mic", false),
        ("missing-key", true),
    ] {
        let mut command = oer_process::command("sudo");
        command.args([
            "-n",
            "/usr/local/libexec/open-radio-bluetooth",
            "security-failure",
            "--adapter",
            &adapter.to_string(),
            "--peer",
            "invalid",
            "--failure",
            failure,
        ]);
        if read_version {
            command.arg("--read-version-before-disconnect");
        }
        let output = oer_process::output(&mut command, Some(Duration::from_secs(5)))?;
        require_security_failure_admission(output.status.code(), &output.stderr)?;
    }
    Ok(())
}

fn require_security_failure_admission(code: Option<i32>, stderr: &[u8]) -> crate::Result<()> {
    if code == Some(2)
        && String::from_utf8_lossy(stderr)
            .contains("invalid value 'invalid' for '--peer <PEER>': peer must be a public address")
    {
        Ok(())
    } else {
        Err("security-failure helper is not admitted without a password; rerun cargo stand fixture install --provider linux-bluetooth with the selected --adapter".into())
    }
}

pub fn connect_profile_in(
    output: &Path,
    adapter: Adapter,
    peer: model::PeerAddress,
    hold_ms: u16,
    termination: BluetoothPeripheralTermination,
    encrypted: bool,
    key_refresh: bool,
) -> crate::Result<model::ConnectionReset> {
    if key_refresh && !encrypted {
        return Err("key refresh requires encrypted ACL".into());
    }
    fs::create_dir_all(output)?;
    if hold_ms > 5_000 {
        return Err("connection hold must be at most 5000 ms".into());
    }
    let mut command = oer_process::command("sudo");
    command
        .args([
            "-n",
            "/usr/local/libexec/open-radio-bluetooth",
            "connect-reset",
            "--adapter",
            &adapter.to_string(),
            "--peer",
            &peer.to_string(),
            "--hold-ms",
            &hold_ms.to_string(),
            "--termination",
            match termination {
                BluetoothPeripheralTermination::PeerReset => "peer-reset",
                BluetoothPeripheralTermination::PeerRfkill => "peer-rfkill",
                BluetoothPeripheralTermination::TargetDisconnect => "target-disconnect",
                BluetoothPeripheralTermination::TargetReset => "target-reset",
            },
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join(HELPER_STDOUT))?))
        .stderr(Stdio::from(fs::File::create(output.join("helper.stderr"))?));
    if key_refresh {
        command.arg("--key-refresh");
    }
    if encrypted {
        command.arg("--encrypted");
    }
    let result = (|| -> crate::Result<model::ConnectionReset> {
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        let status = child.wait_timeout(Some(Duration::from_secs(45)))?;
        let report: model::ConnectionReset = serde_json::from_slice(&fs::read(output.join(HELPER_STDOUT))?)
            .map_err(|error| format!("invalid connect-reset report ({error}); rerun cargo stand fixture install --provider linux-bluetooth"))?;
        if !status.success()
            || !report.passed_profile(adapter, peer, hold_ms, termination, encrypted, key_refresh)
        {
            return Err(format!(
                "connect-reset failed or incomplete: {}",
                report.errors.join("; ")
            )
            .into());
        }
        Ok(report)
    })();
    let report = helper_record(output, || {
        model::ConnectionReset::new(adapter, peer, hold_ms, termination)
    })?;
    let summary = serde_json::json!({
        "schema": 1, "operation": "connect-reset", "adapter": adapter.to_string(),
        "termination": termination,
        "output": output, "passed": result.is_ok(),
        "rf_loss_verified": result.is_ok() && report.peer_rfkill_blocked,
        "error": result.as_ref().err().map(ToString::to_string), "helper": report,
    });
    oer_durable::atomic_json(&output.join("result.json"), &summary)?;
    result.map_err(|error| format!("{error}; evidence: {}", output.display()).into())
}

/// Execute the finite key failure probe through the installed, leased helper.
pub fn security_failure_in(
    output: &Path,
    adapter: Adapter,
    peer: model::PeerAddress,
    failure: oer_hil_protocol::bluetooth::BluetoothSecurityFailure,
    read_version_before_disconnect: bool,
) -> crate::Result<model::security_failure::Report> {
    use oer_hil_protocol::bluetooth::BluetoothSecurityFailure as Failure;
    fs::create_dir_all(output)?;
    let mut command = oer_process::command("sudo");
    command
        .args([
            "-n",
            "/usr/local/libexec/open-radio-bluetooth",
            "security-failure",
            "--adapter",
            &adapter.to_string(),
            "--peer",
            &peer.to_string(),
            "--failure",
            match failure {
                Failure::MissingKey => "missing-key",
                Failure::WrongKey => "wrong-key",
                Failure::MissingRefreshKey => "missing-refresh-key",
                Failure::ActiveDataMic => "active-data-mic",
            },
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join(HELPER_STDOUT))?))
        .stderr(Stdio::from(fs::File::create(output.join("helper.stderr"))?));
    if read_version_before_disconnect {
        command.arg("--read-version-before-disconnect");
    }
    let result = (|| -> crate::Result<model::security_failure::Report> {
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        let status = child.wait_timeout(Some(Duration::from_secs(35)))?;
        let report: model::security_failure::Report =
            serde_json::from_slice(&fs::read(output.join(HELPER_STDOUT))?).map_err(|error|
                format!("security-failure helper returned no valid report ({error}); exit {status}; inspect {}", output.join("helper.stderr").display()))?;
        if !status.success()
            || !report.passed(adapter, peer, failure)
            || report.read_version_before_disconnect != read_version_before_disconnect
        {
            return Err(
                format!("security failure probe failed or incomplete: {:?}", report).into(),
            );
        }
        Ok(report)
    })();
    let observed = helper_record(output, || {
        model::security_failure::Report::new(adapter, peer, failure)
    })?;
    oer_durable::atomic_json(
        &output.join("result.json"),
        &serde_json::json!({
            "schema": 1, "failure": failure, "read_version_before_disconnect":read_version_before_disconnect, "passed": result.is_ok(), "helper": observed,
            "error": result.as_ref().err().map(ToString::to_string),
        }),
    )?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn security_admission_rejects_password_requirement_and_unrelated_cli_errors() {
        let rejected_peer =
            b"error: invalid value 'invalid' for '--peer <PEER>': peer must be a public address";
        assert!(require_security_failure_admission(Some(2), rejected_peer).is_ok());
        assert!(
            require_security_failure_admission(Some(1), b"sudo: a password is required").is_err()
        );
        assert!(require_security_failure_admission(Some(0), rejected_peer).is_err());
        assert!(
            require_security_failure_admission(Some(2), b"unknown command security-failure")
                .is_err()
        );
    }
    #[test]
    fn absent_or_partial_helper_evidence_cannot_pass() {
        assert!(
            checked_report(
                true,
                Adapter(0),
                DtmVersion::V2,
                DtmProfile::ReceiveTransmit,
                b""
            )
            .is_err()
        );
        let report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit);
        assert!(
            checked_report(
                true,
                Adapter(0),
                DtmVersion::V2,
                DtmProfile::ReceiveTransmit,
                &serde_json::to_vec(&report).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn preflight_rejects_stale_or_extended_helper_capabilities() {
        let expected = format!("{}\n", model::HELPER_CAPABILITIES);
        assert!(require_helper_capabilities(true, expected.as_bytes()).is_ok());
        assert!(require_helper_capabilities(false, expected.as_bytes()).is_err());
        assert!(require_helper_capabilities(true, b"schema=4\n").is_err());
        assert!(
            require_helper_capabilities(true, format!("{expected}extra\n").as_bytes()).is_err()
        );
    }

    #[test]
    fn checked_report_rejects_other_profile_old_schema_and_failed_exit() {
        let mut report = Check::new(Adapter(0), DtmVersion::V1, DtmProfile::ReceiveTransmit);
        report.address = Some("peer".into());
        report.version = Some("version".into());
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(true);
        report.dtm_v1_advertised = true;
        report.dtm_v2_advertised = true;
        report.rx_started = true;
        report.rx_packets = Some(0);
        report.tx_started = true;
        report.tx_test_end = true;
        report.restored = true;
        let bytes = serde_json::to_vec(&report).unwrap();
        assert!(
            checked_report(
                true,
                Adapter(0),
                DtmVersion::V1,
                DtmProfile::ReceiveTransmit,
                &bytes
            )
            .is_ok()
        );
        assert!(
            checked_report(
                false,
                Adapter(0),
                DtmVersion::V1,
                DtmProfile::ReceiveTransmit,
                &bytes
            )
            .is_err()
        );
        assert!(
            checked_report(
                true,
                Adapter(0),
                DtmVersion::V2,
                DtmProfile::ReceiveTransmit,
                &bytes
            )
            .is_err()
        );
        let mut old = serde_json::to_value(&report).unwrap();
        old["schema"] = 1.into();
        old.as_object_mut().unwrap().remove("dtm_version");
        old.as_object_mut().unwrap().remove("dtm_v1_advertised");
        assert!(
            checked_report(
                true,
                Adapter(0),
                DtmVersion::V2,
                DtmProfile::ReceiveTransmit,
                &serde_json::to_vec(&old).unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn helper_failure_exposes_the_controller_error() {
        let mut report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit);
        report.errors.push("RX rejected: command disallowed".into());
        let error = checked_report(
            false,
            Adapter(0),
            DtmVersion::V2,
            DtmProfile::ReceiveTransmit,
            &serde_json::to_vec(&report).unwrap(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("RX rejected: command disallowed")
        );
    }
}
