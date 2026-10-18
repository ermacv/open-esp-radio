//! Fixed provider launchers bind a lease and committed generation before exec.

use std::process::ExitCode;

use crate::{ArtifactRole, Provider};

#[cfg(target_os = "linux")]
mod handoff;

#[derive(Clone, Copy)]
pub enum LaunchTarget {
    Network,
    Probe,
}

impl LaunchTarget {
    pub(crate) fn provider(self) -> Provider {
        match self {
            Self::Network | Self::Probe => Provider::LinuxNet,
        }
    }

    pub(crate) fn role(self) -> ArtifactRole {
        match self {
            Self::Network => ArtifactRole::NetworkHelper,
            Self::Probe => ArtifactRole::ProbeHelper,
        }
    }

    /// Installed path of this target's fixed launcher.
    fn launcher(self) -> &'static str {
        let role = match self {
            Self::Network => ArtifactRole::NetworkLauncher,
            Self::Probe => ArtifactRole::ProbeLauncher,
        };
        self.provider()
            .artifact_specs()
            .iter()
            .find(|spec| spec.role == role)
            .map(|spec| spec.target)
            .expect("every launch target installs its fixed launcher")
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Network => "linux-net",
            Self::Probe => "linux-net-probe",
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_support {
    use std::path::Path;

    use super::LaunchTarget;

    pub(crate) fn run_at(
        root: &Path,
        target: LaunchTarget,
        arguments: &[&str],
    ) -> crate::Result<std::process::ExitStatus> {
        let lease = crate::admission::test_support::admit(root, target.provider())?;
        let helper = lease.artifact(target.role())?;
        Ok(oer_process::command(helper)
            .args(arguments)
            .env(super::handoff::BOUND, target.marker())
            .env(super::handoff::DIRECTORY, lease.generation())
            .status()?)
    }
}

pub fn main(target: LaunchTarget) -> ExitCode {
    match launch(target) {
        Ok(never) => match never {},
        Err(error) => {
            eprintln!("fixture operational admission: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Enter a helper that runs only through its fixed launcher.
///
/// A direct invocation re-enters through the installed launcher and does not
/// return on success. Under the launcher, this adopts the provider lease it
/// passed; keep the returned file for the helper's lifetime. Call this first
/// in `main`.
#[cfg(target_os = "linux")]
pub fn enter(target: LaunchTarget) -> crate::Result<std::fs::File> {
    use std::os::unix::process::CommandExt as _;

    if std::env::var(handoff::BOUND).as_deref() != Ok(target.marker()) {
        let error = oer_process::command(target.launcher())
            .args(std::env::args_os().skip(1))
            .exec();
        return Err(format!("{}: {error}", target.launcher()).into());
    }
    adopt_lease(target)
}

#[cfg(target_os = "linux")]
fn adopt_lease(target: LaunchTarget) -> crate::Result<std::fs::File> {
    use std::{
        fs::OpenOptions,
        os::unix::{fs::MetadataExt as _, fs::OpenOptionsExt as _},
        path::Path,
    };

    let file = handoff::adopt(target.marker())?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.mode() & 0o777 != 0o644
    {
        return Err("inherited provider lease has unsafe ownership or mode".into());
    }
    let provider = target.provider().as_str();
    let expected = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(format!(
            "/var/lib/open-radio/fixture/{provider}/session.lock"
        ))?;
    let expected_metadata = expected.metadata()?;
    if metadata.dev() != expected_metadata.dev() || metadata.ino() != expected_metadata.ino() {
        return Err("inherited descriptor is not the persistent provider lease".into());
    }
    // The shared lock stays with the inherited descriptor.
    let path = format!("/var/lib/open-radio/fixture/{provider}/session.lock");
    Ok(oer_process::lock::FileLock::try_lock(
        file,
        Path::new(&path),
        oer_process::lock::Mode::Shared,
    )?
    .ok_or("the provider lease is being updated")?
    .into_file())
}

#[cfg(target_os = "linux")]
fn launch(target: LaunchTarget) -> crate::Result<std::convert::Infallible> {
    use std::os::unix::process::CommandExt as _;

    let lease = super::admit_system(target.provider())?;
    let helper = lease.artifact(target.role())?;
    handoff::pass(lease.file())?;
    let error = oer_process::command(&helper)
        .args(std::env::args_os().skip(1))
        .env(handoff::BOUND, target.marker())
        .env(handoff::DIRECTORY, lease.generation())
        .exec();
    Err(error.into())
}

#[cfg(not(target_os = "linux"))]
fn launch(_target: LaunchTarget) -> crate::Result<std::convert::Infallible> {
    Err("Linux fixture launcher requires Linux".into())
}

#[cfg(test)]
mod tests {
    use super::LaunchTarget;
    use crate::ArtifactRole;

    #[test]
    fn every_target_enters_through_its_installed_launcher() {
        for target in [LaunchTarget::Network, LaunchTarget::Probe] {
            let launcher = target.launcher();
            let spec = target
                .provider()
                .artifact_specs()
                .iter()
                .find(|spec| spec.target == launcher)
                .unwrap();
            assert!(spec.role.is_launcher());
            assert_ne!(spec.role, ArtifactRole::NetworkHelper);
        }
    }
}
