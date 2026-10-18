//! Fail-closed operational admission for one committed provider generation.

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::{self, File, OpenOptions},
        os::unix::{fs::MetadataExt as _, fs::OpenOptionsExt as _},
        path::{Component, Path, PathBuf},
    };

    use crate::{ArtifactRole, InstallResult, InstallState, Provider, RECEIPT_SCHEMA, Result};

    pub struct OperationalLease {
        /// The installation lease's shared lock, released when the lease
        /// drops; the launcher hands its descriptor to the helper it executes.
        lock: oer_process::lock::FileLock,
        expected_uid: u32,
        expected_gid: u32,
        generation: PathBuf,
        receipt: InstallResult,
    }

    impl OperationalLease {
        pub fn artifact(&self, role: ArtifactRole) -> Result<PathBuf> {
            let artifact = self
                .receipt
                .artifacts
                .iter()
                .find(|artifact| artifact.role == role)
                .ok_or_else(|| format!("committed generation has no {role:?} artifact"))?;
            let expected_name = match role {
                ArtifactRole::NetworkLauncher => "open-radio-net-launcher",
                ArtifactRole::NetworkHelper => "open-radio-net",
                ArtifactRole::ProbeLauncher => "open-radio-probe-launcher",
                ArtifactRole::ProbeHelper => "open-radio-probe",
                ArtifactRole::Hostapd => "open-radio-hostapd",
                ArtifactRole::HostapdProvenance => "open-radio-hostapd.json",
            };
            if artifact.file_name != expected_name {
                return Err(format!(
                    "recovery-required: committed {role:?} artifact has an unexpected name"
                )
                .into());
            }
            let expected_mode = if role == ArtifactRole::HostapdProvenance {
                0o444
            } else {
                0o555
            };
            if artifact.mode != expected_mode {
                return Err(format!(
                    "recovery-required: committed {role:?} artifact has an unexpected mode"
                )
                .into());
            }
            let path = self.generation.join(&artifact.file_name);
            require_file(&path, artifact.mode, self.expected_uid, self.expected_gid)?;
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 != artifact.size_bytes
                || oer_durable::sha256_bytes(&bytes) != artifact.sha256
            {
                return Err(format!(
                    "recovery-required: committed artifact identity differs at {}",
                    path.display()
                )
                .into());
            }
            Ok(path)
        }

        pub(crate) fn file(&self) -> &File {
            self.lock.file()
        }

        pub fn generation(&self) -> &Path {
            &self.generation
        }
    }

    pub fn admit_system(provider: Provider) -> Result<OperationalLease> {
        admit_at(Path::new("/"), provider, 0, 0)
    }

    fn admit_at(
        root: &Path,
        provider: Provider,
        expected_uid: u32,
        expected_gid: u32,
    ) -> Result<OperationalLease> {
        let fixture_root = map(root, Path::new("/var/lib/open-radio/fixture"));
        let state_root = fixture_root.join(provider.as_str());
        require_directory(&fixture_root, 0o755, expected_uid, expected_gid)?;
        require_directory(&state_root, 0o755, expected_uid, expected_gid)?;

        let lease_path = state_root.join("session.lock");
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&lease_path)
            .map_err(|error| {
                format!(
                    "fixture provider {provider} has no safe persistent installation lease at {} ({error}); reinstall it with cargo stand fixture install --provider {provider}",
                    lease_path.display()
                )
            })?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != expected_uid
            || metadata.gid() != expected_gid
            || metadata.mode() & 0o777 != 0o644
        {
            return Err(format!(
                "fixture provider {provider} installation lease has unsafe ownership or mode"
            )
            .into());
        }
        let lock = oer_process::lock::FileLock::try_lock(
            file,
            &lease_path,
            oer_process::lock::Mode::Shared,
        )?
        .ok_or_else(|| format!("fixture provider {provider} is being updated"))?;

        let journal = state_root.join("transaction.json");
        match fs::symlink_metadata(&journal) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "recovery-required: cannot inspect provider {provider} transaction state: {error}"
                )
                .into());
            }
            Ok(_) => {
                return Err(format!(
                    "recovery-required: provider {provider} has an unfinished installation transaction"
                )
                .into());
            }
        }

        let generations = state_root.join("generations");
        require_directory(&generations, 0o755, expected_uid, expected_gid)?;
        let generation_name = read_current(&state_root.join("current"))?;
        if generation_name.len() != 64
            || !generation_name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("recovery-required: selected generation identity is invalid".into());
        }
        let generation = generations.join(&generation_name);
        require_directory(&generation, 0o555, expected_uid, expected_gid)?;

        let receipt_path = state_root.join("receipt.json");
        require_file(&receipt_path, 0o444, expected_uid, expected_gid)?;
        let receipt: InstallResult = serde_json::from_slice(&fs::read(&receipt_path)?)
            .map_err(|error| format!("recovery-required: invalid provider receipt: {error}"))?;
        if receipt.schema != RECEIPT_SCHEMA
            || receipt.provider != provider
            || receipt.generation != generation_name
            || receipt.state != InstallState::SoftwareVerified
            || !receipt.activation_committed
            || !receipt.software_verified
            || receipt.primary_error.is_some()
            || receipt.recovery_error.is_some()
        {
            return Err(format!(
                "recovery-required: provider {provider} receipt does not prove the selected committed generation"
            )
            .into());
        }

        Ok(OperationalLease {
            lock,
            expected_uid,
            expected_gid,
            generation,
            receipt,
        })
    }

    fn read_current(path: &Path) -> Result<String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "recovery-required: provider generation selector is unavailable at {}: {error}",
                path.display()
            )
        })?;
        if !metadata.file_type().is_symlink() {
            return Err("recovery-required: provider generation selector is not a symlink".into());
        }
        let target = fs::read_link(path)?;
        let mut components = target.components();
        if components.next() != Some(Component::Normal("generations".as_ref())) {
            return Err(
                "recovery-required: provider generation selector has an unexpected target".into(),
            );
        }
        let Some(Component::Normal(generation)) = components.next() else {
            return Err("recovery-required: provider generation selector has no generation".into());
        };
        if components.next().is_some() {
            return Err(
                "recovery-required: provider generation selector escapes generations".into(),
            );
        }
        Ok(generation
            .to_str()
            .ok_or("recovery-required: provider generation is not UTF-8")?
            .to_owned())
    }

    fn require_directory(path: &Path, mode: u32, uid: u32, gid: u32) -> Result<()> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "recovery-required: cannot inspect installation directory {}: {error}",
                path.display()
            )
        })?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != uid
            || metadata.gid() != gid
            || metadata.mode() & 0o777 != mode
            || metadata.mode() & 0o022 != 0
        {
            return Err(format!(
                "recovery-required: installation directory has unsafe type, ownership or mode: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    }

    fn require_file(path: &Path, mode: u32, uid: u32, gid: u32) -> Result<()> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "recovery-required: cannot inspect installed file {}: {error}",
                path.display()
            )
        })?;
        if !metadata.file_type().is_file()
            || metadata.uid() != uid
            || metadata.gid() != gid
            || metadata.mode() & 0o777 != mode
        {
            return Err(format!(
                "recovery-required: installed file has unsafe type, ownership or mode: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    }

    fn map(root: &Path, absolute: &Path) -> PathBuf {
        if root == Path::new("/") {
            absolute.to_owned()
        } else {
            root.join(absolute.strip_prefix("/").expect("fixed absolute path"))
        }
    }

    #[cfg(test)]
    pub(crate) mod test_support {
        use super::*;

        pub(crate) fn admit(root: &Path, provider: Provider) -> Result<OperationalLease> {
            admit_at(
                root,
                provider,
                rustix::process::geteuid().as_raw(),
                rustix::process::getegid().as_raw(),
            )
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{OperationalLease, admit_system};

#[cfg(not(target_os = "linux"))]
pub struct OperationalLease;

#[cfg(not(target_os = "linux"))]
pub fn admit_system(_provider: crate::Provider) -> crate::Result<OperationalLease> {
    Err("Linux fixture software admission requires Linux".into())
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) use linux::test_support;
