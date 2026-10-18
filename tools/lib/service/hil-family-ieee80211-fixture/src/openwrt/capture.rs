//! Scoped OpenWrt packet capture on an owned monitor or an existing interface.
//! Readiness and stop use process events; only created interfaces are removed.
use crate::Result;
use crate::capture_process;
use oer_hil_lab::config::OpenWrtConfig;
use oer_process::CommandExt as _;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;

struct RemoteInterface {
    create_interface: bool,
    interface: String,
    directory: String,
}

impl RemoteInterface {
    fn new(interface: String) -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            create_interface: true,
            interface,
            directory: format!(
                "/tmp/oer-capture-{}-{epoch}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        })
    }

    fn capture(&self) -> String {
        format!("{}/capture.pcap", self.directory)
    }

    fn start_script(
        &self,
        config: &OpenWrtConfig,
        filter: &str,
        immediate: bool,
        duration: Duration,
    ) -> String {
        let monitor = &self.interface;
        let directory = &self.directory;
        let remote = self.capture();
        let lifetime = duration.saturating_add(Duration::from_secs(120));
        let mode = if immediate && !self.create_interface {
            "--immediate-mode -s 128"
        } else if immediate {
            "--immediate-mode -s 512"
        } else {
            "-s 128"
        };
        let program = format!(
            "tcpdump -i {monitor} -n {mode} -U -w {remote} {}",
            capture_process::quote(filter)
        );
        if !self.create_interface {
            let controlled = capture_process::controlled(&program, ":");
            // Bound the remote file while tcpdump is running, not merely
            // when downloading it. Exceeding the limit terminates capture;
            // the terminal status then rejects incomplete evidence.
            let script =
                format!("set -eu; ulimit -f 65536; umask 077; mkdir {directory}; {controlled}");
            return format!(
                "LC_ALL=C timeout -s TERM {} sh -c {}",
                lifetime.as_secs(),
                capture_process::quote(&script)
            );
        }
        let controlled = capture_process::controlled(&program, "cleanup");
        let script = format!(
            "set -eu; \
             if iw dev {monitor} info >/dev/null 2>&1; then echo 'monitor interface already exists' >&2; exit 1; fi; \
             wiphy=$(iw dev {wireless} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
             test -n \"$wiphy\"; \
             umask 077; mkdir {directory}; \
             cleanup() {{ iw dev {monitor} del >/dev/null 2>&1 || true; }}; \
             trap cleanup EXIT; \
             trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM; \
             iw phy \"$wiphy\" interface add {monitor} type monitor; \
             ip link set {monitor} up; \
             {controlled}",
            wireless = config.wireless_interface,
        );
        format!(
            "LC_ALL=C timeout -s TERM {} sh -c {}",
            lifetime.as_secs(),
            capture_process::quote(&script)
        )
    }

    fn cleanup_script(&self) -> String {
        if !self.create_interface {
            return format!(
                "if test -d {0}; then rm -f {1}; rmdir {0}; fi",
                self.directory,
                self.capture()
            );
        }
        // The private directory is acquired only after rejecting an existing
        // interface. A failed preflight/spawn therefore cannot delete it.
        format!(
            "set -eu; if test -d {directory}; then \
             if iw dev {monitor} info >/dev/null 2>&1; then iw dev {monitor} del; fi; \
             rm -f {capture}; rmdir {directory}; fi",
            directory = self.directory,
            monitor = self.interface,
            capture = self.capture(),
        )
    }
}

pub(crate) struct RemoteCapture {
    ssh_target: String,
    remote: RemoteInterface,
    output: PathBuf,
    child: Option<capture_process::Capture>,
}

impl RemoteCapture {
    pub(crate) fn output_path(&self) -> &Path {
        &self.output
    }

    pub(crate) fn start_monitor(
        config: &OpenWrtConfig,
        output: PathBuf,
        filter: &str,
        immediate: bool,
        duration: Duration,
    ) -> Result<Self> {
        if config.read_only {
            return Err("read-only OpenWrt fixture forbids creating monitors".into());
        }
        let monitor = config
            .monitor_interface
            .clone()
            .ok_or("OpenWrt capture requires station_fixture.monitor_interface")?;
        oer_process::check_cancelled()?;
        let remote = RemoteInterface::new(monitor)?;
        let script = remote.start_script(config, filter, immediate, duration);
        // Own cleanup before starting the remote process or waiting for readiness.
        let mut owner = Self {
            ssh_target: config.ssh_target.clone(),
            remote,
            output,
            child: None,
        };
        owner.child = Some(capture_process::Capture::start(
            &mut oer_stand_ssh::command(&config.ssh_target, &script),
            format!("tcpdump: listening on {},", owner.remote.interface),
            duration.saturating_add(Duration::from_secs(120)),
        )?);
        Ok(owner)
    }

    /// A dedicated idle PHY; never borrows or retunes another interface.
    pub(crate) fn start_independent(
        config: &oer_hil_lab::config::AirObserverConfig,
        geometry: crate::channel::Geometry,
        filter: &str,
        snapshot_length: SnapshotLength,
        output: PathBuf,
        duration: Duration,
    ) -> Result<Self> {
        let remote = RemoteInterface::new(config.interface.clone())?;
        let script =
            remote.independent_script(config, geometry, filter, snapshot_length, duration)?;
        let mut owner = Self {
            ssh_target: config.ssh_target.clone(),
            remote,
            output,
            child: None,
        };
        owner.child = Some(capture_process::Capture::start(
            &mut oer_stand_ssh::command(&owner.ssh_target, &script),
            format!("tcpdump: listening on {},", owner.remote.interface),
            duration.saturating_add(Duration::from_secs(120)),
        )?);
        let observed = oer_stand_ssh::command(
            &owner.ssh_target,
            &format!("iw dev {} info", owner.remote.interface),
        )
        .supervised_output()?;
        if !observed.status.success()
            || crate::channel::Geometry::parse(std::str::from_utf8(&observed.stdout)?)? != geometry
        {
            return Err("independent monitor actual channel differs from the AP".into());
        }
        Ok(owner)
    }

    pub(crate) fn start_managed(
        config: &OpenWrtConfig,
        interface: &str,
        output: PathBuf,
        filter: &str,
        duration: Duration,
    ) -> Result<Self> {
        let mut remote = RemoteInterface::new(interface.to_owned())?;
        remote.create_interface = false;
        let script = remote.start_script(config, filter, true, duration);
        let mut owner = Self {
            ssh_target: config.ssh_target.clone(),
            remote,
            output,
            child: None,
        };
        owner.child = Some(capture_process::Capture::start(
            &mut oer_stand_ssh::command(&config.ssh_target, &script),
            format!("tcpdump: listening on {},", interface),
            duration.saturating_add(Duration::from_secs(120)),
        )?);
        Ok(owner)
    }

    pub(crate) fn finish_capture(&mut self) -> Result<(u64, u64)> {
        let child = self.child.take().expect("packet capture owns its child");
        let output = child.finish()?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "OpenWrt packet capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = summary_value(&summary, "packets captured")
            .ok_or("OpenWrt packet capture omitted its packet count")?;
        let kernel_dropped = summary_value(&summary, "packets dropped by kernel")
            .ok_or("tcpdump omitted its drop count")?;
        copy_remote(&self.ssh_target, &self.remote.capture(), &self.output)?;
        if kernel_dropped != 0 {
            return Err(crate::Error::new(format!(
                "OpenWrt packet capture dropped {kernel_dropped} packets in its capture socket"
            ))
            .into());
        }
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(crate::Error::new(format!(
                "OpenWrt packet capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            ))
            .into());
        }
        Ok((captured_frames, kernel_dropped))
    }
}

impl Drop for RemoteCapture {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            drop(self.child.take());
            let cleanup = self.remote.cleanup_script();
            oer_hil_workload::fixture::cleanup::command(
                "clean up OpenWrt capture",
                &mut oer_stand_ssh::command(&self.ssh_target, &cleanup),
            );
        });
    }
}

fn copy_remote(target: &str, remote: &str, local: &Path) -> Result<()> {
    let file = File::create(local)?;
    let status = oer_stand_ssh::command(target, &format!("cat {remote}"))
        .stdout(Stdio::from(file))
        .supervised_status()?;
    if !status.success() {
        return Err(
            crate::Error::new("cannot copy OpenWrt packet capture to the run directory").into(),
        );
    }
    Ok(())
}

fn summary_value(summary: &str, suffix: &str) -> Option<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_suffix(suffix)?.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests;

impl RemoteInterface {
    fn independent_script(
        &self,
        config: &oer_hil_lab::config::AirObserverConfig,
        geometry: crate::channel::Geometry,
        filter: &str,
        snapshot_length: SnapshotLength,
        duration: Duration,
    ) -> Result<String> {
        let interface = &self.interface;
        let phy = &config.phy;
        let directory = &self.directory;
        let controlled = capture_process::controlled(
            &format!(
                "tcpdump -B 4096 -i {interface} -n -s {} -U -w {} {}",
                snapshot_length.bytes(),
                self.capture(),
                capture_process::quote(filter)
            ),
            "cleanup",
        );
        let script = format!(
            r#"set -eu
command -v tcpdump >/dev/null
iw phy {phy} info | grep -q '^[[:space:]]*\* monitor$'
interfaces=$(iw dev | awk -v wanted={phy} '/phy#/ {{ p="phy" substr($0,5) }} /Interface/ && p==wanted {{ print $2 }}')
if [ -n "$interfaces" ]; then echo 'independent observer PHY is not idle' >&2; exit 1; fi
if iw dev {interface} info >/dev/null 2>&1; then echo 'observer interface already exists' >&2; exit 1; fi
umask 077
mkdir {directory}
cleanup() {{ iw dev {interface} del >/dev/null 2>&1 || true; }}
trap cleanup EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
iw phy {phy} interface add {interface} type monitor
ip link set {interface} up
iw dev {interface} set freq {frequency} {width}
ulimit -f 131072
{controlled}
"#,
            frequency = geometry.frequency,
            width = geometry.iw_width()?
        );
        Ok(format!(
            "LC_ALL=C timeout -s TERM {} sh -c {}",
            duration.saturating_add(Duration::from_secs(120)).as_secs(),
            capture_process::quote(&script)
        ))
    }
}

/// How much of each frame the independent observer retains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotLength {
    /// MAC headers and short control frames, enough for traffic analysis.
    Headers,
    /// Complete management frames, including every information element.
    Complete,
}

impl SnapshotLength {
    const fn bytes(self) -> u16 {
        match self {
            Self::Headers => 128,
            Self::Complete => 2304,
        }
    }
}
