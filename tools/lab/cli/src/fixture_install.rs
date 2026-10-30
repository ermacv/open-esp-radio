//! `cargo stand fixture install`: offline planning and unprivileged
//! preparation of a Linux fixture software bundle, then the foreground sudo
//! handoff to the privileged installer under a stand-maintenance lease.

use std::path::Path;

use oer_lab_fixture::{INSTALLER_BINARY, Provider, build_plan, inspect_system, prepare};

use crate::Result;

pub(crate) fn run(
    root: &Path,
    provider: Provider,
    dry_run: bool,
    adapters: &[String],
) -> Result<()> {
    let plan = build_plan(root, provider, adapters)?;
    if dry_run {
        let mut preview = serde_json::to_value(&plan)?;
        let fields = preview
            .as_object_mut()
            .ok_or("fixture plan must be an object")?;
        fields.insert(
            "existing_installation".into(),
            serde_json::to_value(inspect_system(provider)?)?,
        );
        fields.insert(
            "pending_privileged_checks".into(),
            serde_json::json!([
                "existing sudoers policy presence, ownership and mode",
                "candidate and effective sudoers policy validation"
            ]),
        );
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        require_unprivileged(rustix::process::geteuid().as_raw())?;
        // All downloads, patching and compilation happen without root, before
        // the installer takes terminal control for its narrow system writes.
        if provider == Provider::LinuxNet {
            oer_lab_router::hostapd::build(root)?;
            oer_lab_router::udhcpc::build(root)?;
        }

        let provider_binaries: &[&str] = match provider {
            Provider::LinuxNet => &[
                "open-radio-net-launcher",
                "open-radio-probe-launcher",
                "open-radio-probe",
                "open-radio-net-client",
            ],
            Provider::LinuxBluetooth => &["open-radio-bluetooth-launcher", "open-radio-bluetooth"],
        };
        let mut build = oer_os_process::command(oer_os_toolchain::cargo_program());
        build.current_dir(root).args([
            "build",
            "--locked",
            "-p",
            "oer-tool-lab-fixture-install",
            "-p",
            "oer-tool-lab-fixture-helpers",
        ]);
        for binary in provider_binaries {
            build.args(["--bin", binary]);
        }
        // The root workspace's own target directory: the binaries reuse
        // `target/debug` of every other root build.
        build
            .args(["--bin", INSTALLER_BINARY])
            .env("CARGO_TARGET_DIR", root.join("target"));
        oer_os_process::run(&mut build)?;
        let operator = current_operator()?;
        let bundle = prepare(root, provider, &operator, &plan.allowed_bluetooth_adapters)?;
        // Queue behind the runs using the provider; runs queued later wait
        // for the installation. Nothing supervises this lease: the operator
        // may take a while to answer sudo, and a partial installation is
        // worse than a late one.
        eprintln!(
            "stand: queueing for {}",
            oer_lab_fixture::resource(provider)
        );
        let (mut lease, owner) = install_lease(provider)?;
        // sudo runs in this foreground process group with the terminal's
        // standard streams, so it owns password echo, input and job control,
        // and its exit status is the command's.
        let status = oer_os_process::command("sudo")
            .arg(root.join("target/debug").join(INSTALLER_BINARY))
            .args(["--provider", provider.as_str(), "--bundle"])
            .arg(bundle)
            .status()
            .map_err(|error| format!("cannot start the Linux fixture installer: {error}"))?;
        // An installation that did not finish leaves the stand under
        // maintenance until a person releases it.
        if status.success() {
            lease.administer(oer_lab_service::protocol::Request::EndMaintenance {
                mac: oer_lab_arbiter::STAND_SERVICE.to_owned(),
            })?;
        } else {
            eprintln!(
                "stand: the installation failed; the stand stays under maintenance by {owner} \
                 until `cargo stand devices release --stand`"
            );
        }
        drop(lease);
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, provider, adapters, plan);
        Err("fixture installation requires Linux".into())
    }
}

/// Take the stand for installing `provider`: stand maintenance, served
/// before every waiting request, and every run that uses the provider is
/// preempted, its client told why. The stand stays under the owner's
/// maintenance until the installation succeeded: the installer itself runs
/// in this process, which the service does not stop, since a partial
/// installation is worse than a late one.
#[cfg(target_os = "linux")]
fn install_lease(provider: Provider) -> Result<(oer_lab_service::client::Client, String)> {
    let owner = oer_lab_arbiter::owner::from_environment()?.to_string();
    let reason = format!(
        "fixture maintenance: {} install by {owner}",
        provider.as_str()
    );
    let mut client = crate::service::client()?;
    client.acquire(
        oer_lab_service::protocol::LeaseRequest {
            owner: owner.clone(),
            work: format!("fixture install --provider {}", provider.as_str()),
            scenarios: Vec::new(),
            run: None,
            claims: vec![oer_lab_arbiter::claims::Claim::exclusive(
                oer_lab_fixture::resource(provider),
            )],
            maintenance: Some(reason.clone()),
            divisible: false,
        },
        &mut |message| eprintln!("stand: {message}"),
    )?;
    client.administer(oer_lab_service::protocol::Request::Maintain {
        mac: oer_lab_arbiter::STAND_SERVICE.to_owned(),
        owner: owner.clone(),
        reason,
    })?;
    Ok((client, owner))
}

#[cfg(target_os = "linux")]
fn current_operator() -> Result<String> {
    let output = oer_os_process::command("/usr/bin/id").arg("-un").output()?;
    if !output.status.success() {
        return Err("cannot resolve the non-root fixture operator".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn require_unprivileged(uid: u32) -> Result<()> {
    if uid == 0 {
        return Err(
            "run cargo stand fixture install as the non-root operator; sudo is used only for the final apply"
                .into(),
        );
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn elevated_invocation_is_rejected_before_preparation() {
        assert!(super::require_unprivileged(0).is_err());
        assert!(super::require_unprivileged(1000).is_ok());
    }
}
