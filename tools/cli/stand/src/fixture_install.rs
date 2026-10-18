//! `cargo stand fixture install`: offline planning and unprivileged
//! preparation of a Linux fixture software bundle, then the foreground sudo
//! handoff to the privileged installer under a stand-maintenance lease.

use std::path::Path;

use oer_stand_fixture_install::{INSTALLER_BINARY, Provider, build_plan, prepare};

use crate::Result;

pub(crate) fn run(root: &Path, provider: Provider, dry_run: bool) -> Result<()> {
    let plan = build_plan(root, provider)?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        require_unprivileged(rustix::process::geteuid().as_raw())?;
        // All downloads, patching and compilation happen without root, before
        // the installer takes terminal control for its narrow system writes.
        if provider == Provider::LinuxNet {
            oer_stand_hostapd::build(root)?;
        }

        let provider_binaries: &[&str] = match provider {
            Provider::LinuxNet => &[
                "open-radio-net-launcher",
                "open-radio-probe-launcher",
                "open-radio-probe",
            ],
        };
        let mut build = oer_process::command(oer_toolchain::cargo_program());
        build.current_dir(root).args([
            "build",
            "--locked",
            "-p",
            "oer-stand-fixture-install",
            "-p",
            "oer-hil-fixture",
        ]);
        for binary in provider_binaries {
            build.args(["--bin", binary]);
        }
        // The root workspace's own target directory: the binaries reuse
        // `target/debug` of every other root build.
        build
            .args(["--bin", INSTALLER_BINARY])
            .env("CARGO_TARGET_DIR", root.join("target"));
        oer_process::run(&mut build)?;
        let operator = current_operator()?;
        let bundle = prepare(root, provider, &operator)?;
        // Queue behind the runs using the provider; runs queued later wait
        // for the installation. Nothing supervises this lease: the operator
        // may take a while to answer sudo, and a partial installation is
        // worse than a late one.
        eprintln!(
            "stand: queueing for {}",
            oer_stand_fixture_install::resource(provider)
        );
        let grant = install_grant(provider)?;
        // sudo runs in this foreground process group with the terminal's
        // standard streams, so it owns password echo, input and job control,
        // and its exit status is the command's.
        let status = oer_process::command("sudo")
            .arg(root.join("target/debug").join(INSTALLER_BINARY))
            .args(["--provider", provider.as_str(), "--bundle"])
            .arg(bundle)
            .status()
            .map_err(|error| format!("cannot start the Linux fixture installer: {error}"))?;
        drop(grant);
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, provider, plan);
        Err("fixture installation requires Linux".into())
    }
}

/// Take the stand for installing `provider`: stand maintenance, served
/// before every waiting request, and every run that uses the provider is
/// preempted with the ordinary cleanup and a notice. The grant keeps later
/// runs of the provider queued until it is dropped.
#[cfg(target_os = "linux")]
fn install_grant(provider: Provider) -> Result<oer_stand_arbiter::Grant> {
    let mut request = oer_stand_arbiter::Request::from_environment(format!(
        "fixture install --provider {}",
        provider.as_str()
    ))?;
    request.claims = vec![oer_stand_claims::Claim::exclusive(
        oer_stand_fixture_install::resource(provider),
    )];
    let reason = format!(
        "fixture maintenance: {} install by {}",
        provider.as_str(),
        request.owner
    );
    oer_stand_arbiter::Arbiter::open()?.acquire_maintenance(&request, &reason)
}

#[cfg(target_os = "linux")]
fn current_operator() -> Result<String> {
    let output = oer_process::command("/usr/bin/id").arg("-un").output()?;
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
