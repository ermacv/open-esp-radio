//! Build exclusion: one mechanism, the foundation's advisory file lock
//! ([`oer_os_process::lock::FileLock`]), for every place builds must not
//! overlap.
//!
//! - [`Staging`]: one build owns a bundle's output; a second build of it
//!   fails at once.
//! - the host's one compile cache ([`crate::compile_cache`]) is used by one
//!   build at a time, from its Cargo run until its outputs are copied out,
//!   the next waiting for it ([`oer_image_toolchain::image::lock_compile_cache`]);
//! - [`slot`]: the host-wide limit on concurrent image compilations, one of
//!   a fixed number of lock files;
//! - the ESP-IDF cache: exclusive while its tree and tools change, shared
//!   while builds read them.
//!
//! The lock is the kernel's `flock`: released when its owner exits, however
//! it exits, so a crashed build never leaves a stale lock behind.
use crate::Result;
use oer_os_process::lock::{FileLock, Mode};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One of the host's image compilation slots below `directory`: each holds
/// one runtime build's fat LTO, about 1.5 GB and one core for minutes, and
/// parallel builds of several checkouts otherwise pushed the host into
/// swap. Waits for a free slot.
pub fn slot(directory: &Path) -> Result<FileLock> {
    let count = slots();
    let paths: Vec<PathBuf> = (0..count)
        .map(|slot| directory.join(format!("{slot}.lock")))
        .collect();
    FileLock::wait_any(
        &paths,
        Mode::Exclusive,
        &format!("waiting for one of the host's {count} image build slots"),
    )
}

/// Image compilations the host runs at once: half its cores, and no more
/// than its memory holds at 2 GB each.
fn slots() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let memory_gb = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|kb| {
                    kb.trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
        })
        .map_or(4, |kb| kb >> 20);
    (cores / 2).min(memory_gb / 2).max(1)
}

/// One build of the bundle in `output`, its only one: the build fills the
/// staging directory, a sibling of `output`, and [`Staging::publish`] moves
/// it into place once the whole build succeeded. A build that fails leaves
/// the bundle published before untouched and its staging directory, with
/// its build log, until the next build of `output` starts.
pub struct Staging {
    output: PathBuf,
    directory: PathBuf,
    _lease: FileLock,
}

impl Staging {
    /// Start the build of `output`; fails at once while another build of it
    /// runs.
    pub fn begin(output: &Path) -> Result<Self> {
        let directory = crate::bundle::staging_directory(output)?;
        let parent = directory
            .parent()
            .ok_or("a staging directory has a parent")?;
        fs::create_dir_all(parent)?;
        let name = output
            .file_name()
            .ok_or("an output names a directory")?
            .to_string_lossy();
        let lease = FileLock::try_acquire(
            &parent.join(format!(".{name}.build.lease")),
            Mode::Exclusive,
        )?
        .ok_or_else(|| format!("another build owns {}", output.display()))?;
        crate::bundle::remove_directory(&directory)?;
        fs::create_dir_all(&directory)?;
        Ok(Self {
            output: output.to_owned(),
            directory,
            _lease: lease,
        })
    }

    /// Where the build writes the bundle's files.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Publish `bundle`, built in [`Staging::directory`], as the bundle in
    /// the output.
    pub fn publish(self, bundle: crate::bundle::ImageBundle) -> Result<crate::bundle::ImageBundle> {
        bundle.publish(&self.output)
    }
}

#[cfg(test)]
mod tests;
