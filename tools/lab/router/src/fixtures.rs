//! The equipment the stand reaches over SSH, the lab router and the air
//! observer, probed for `cargo stand fixtures`: only the LAB service runs
//! these probes ([`probe_remote`]). Probing a host takes an SSH round trip.
#![forbid(unsafe_code)]

use std::{path::Path, time::Duration};

use oer_lab_model::equipment::{Fixture, interfaces};

/// How long one SSH probe may take before the host counts as not answering.
const SSH_TIMEOUT: Duration = Duration::from_secs(8);

/// The value of `key` in section `section` of the stand file at `lab`.
fn stand_text(lab: &Path) -> impl Fn(&str, &str) -> Option<String> {
    let config = std::fs::read_to_string(lab)
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
    move |section: &str, key: &str| {
        config
            .as_ref()
            .and_then(|config| config.get(section)?.get(key)?.as_str())
            .map(str::to_owned)
    }
}

/// The equipment the stand file at `lab` reaches over SSH: the lab router
/// and the air observer.
pub fn probe_remote(lab: &Path) -> Vec<Fixture> {
    let text = stand_text(lab);
    let mut hosts = Vec::new();
    if let Some(target) = text("station_fixture", "ssh_target") {
        hosts.push(("station fixture", target));
    }
    if let Some(target) = text("air_observer", "ssh_target") {
        hosts.push(("air observer", target));
    }
    // SSH probes run side by side: one slow host does not delay the others.
    std::thread::scope(|scope| {
        hosts
            .iter()
            .map(|(role, target)| scope.spawn(move || openwrt(role, target)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|probe| probe.join().ok())
            .collect::<Vec<_>>()
    })
}

fn openwrt(role: &str, target: &str) -> Fixture {
    let mut fixture = Fixture {
        role: role.to_owned(),
        name: target.to_owned(),
        ..Fixture::default()
    };
    let script = r#"cat /proc/sys/kernel/random/boot_id
cat /tmp/sysinfo/model 2>/dev/null
. /etc/openwrt_release 2>/dev/null; echo "$DISTRIB_DESCRIPTION"
cut -d. -f1 /proc/uptime
for i in $(iw dev | awk '$1 == "Interface" {print $2}'); do
  echo "@interface $i"
  iw dev "$i" info | awk '$1 == "type" || $1 == "channel"'
  iw dev "$i" survey dump
done"#;
    let output = crate::ssh::Ssh::new(target).and_then(|ssh| ssh.output(script, SSH_TIMEOUT));
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut lines = text.lines();
            fixture.key = crate::ssh::host_key(lines.next().unwrap_or_default()).ok();
            let model = lines.next().unwrap_or_default().trim();
            let release = lines.next().unwrap_or_default().trim();
            let uptime = lines
                .next()
                .and_then(|uptime| uptime.trim().parse::<u64>().ok())
                .map(|seconds| format!("up {}d {}h", seconds / 86400, seconds % 86400 / 3600))
                .unwrap_or_default();
            fixture.detail = Some(
                [model, release, &uptime]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
            fixture.interfaces = interfaces(&lines.collect::<Vec<_>>().join("\n"));
            fixture.reachable = true;
        }
        Ok(output) => {
            fixture.error = Some(
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .last()
                    .unwrap_or("ssh failed")
                    .to_owned(),
            );
        }
        Err(error) => fixture.error = Some(error.to_string()),
    }
    fixture
}
