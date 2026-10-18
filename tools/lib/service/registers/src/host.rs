use crate::Result;
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub(crate) fn destination(path: &Path) -> Result<PathBuf> {
    if path.is_symlink() {
        return Err("publication destination must not be a symlink".into());
    }
    let parent = path
        .parent()
        .ok_or("output has no parent")?
        .canonicalize()?;
    Ok(parent.join(path.file_name().ok_or("output has no filename")?))
}
pub(crate) fn format(source: &str, edition: &str) -> Result<String> {
    // File-backed pipes avoid deadlock and unbounded diagnostic reader threads.
    let mut input = tempfile::tempfile()?;
    input.write_all(source.as_bytes())?;
    input.seek(SeekFrom::Start(0))?;
    let mut output = tempfile::tempfile()?;
    let mut errors = tempfile::tempfile()?;
    let mut command = oer_process::command("rustfmt");
    command
        .args(["--edition", edition, "--style-edition", edition])
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(errors.try_clone()?));
    let mut child = oer_process::owned::Child::spawn(&mut command)?;
    let status = child.wait_timeout(Some(Duration::from_secs(60)))?;
    if !status.success() {
        errors.seek(SeekFrom::Start(0))?;
        let mut message = String::new();
        errors.take(4096).read_to_string(&mut message)?;
        return Err(format!("rustfmt failed: {status}: {message}").into());
    }
    if output.metadata()?.len() > 128 * 1024 * 1024 {
        return Err("generated source exceeds 128 MiB".into());
    }
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.read_to_string(&mut text)?;
    Ok(text)
}
pub(crate) fn publish(outputs: &[(&PathBuf, String)], check: bool) -> Result<()> {
    oer_process::check_cancelled()?;
    if check {
        let mismatches: Vec<_> = outputs
            .iter()
            .filter(|(p, s)| fs::read(p).ok().as_deref() != Some(s.as_bytes()))
            .map(|(p, _)| p.display().to_string())
            .collect();
        if !mismatches.is_empty() {
            return Err(format!(
                "generated outputs differ or are missing: {}",
                mismatches.join(", ")
            )
            .into());
        }
        return Ok(());
    }
    let mut pending = Vec::new();
    for (path, contents) in outputs {
        if path.is_symlink() {
            return Err("publication destination became a symlink".into());
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("output has no parent")?)?;
        temporary.write_all(contents.as_bytes())?;
        // Keep generated source readable under the ordinary repository policy.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o644))?;
        }
        temporary.as_file().sync_all()?;
        pending.push((path, temporary));
    }
    oer_process::check_cancelled()?;
    for (index, (path, temporary)) in pending.into_iter().enumerate() {
        temporary
            .persist(path)
            .map_err(|e| format!("incomplete publication after {index} files: {e}"))?;
        fs::File::open(path.parent().ok_or("output has no parent")?)?.sync_all()?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_and_failed_preparation_do_not_write_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("missing/b");
        fs::write(&a, "old").unwrap();
        let outputs = [(&a, "new".into()), (&b, "new".into())];
        assert!(publish(&outputs, true).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert!(publish(&outputs, false).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert!(!b.exists());
    }
}
