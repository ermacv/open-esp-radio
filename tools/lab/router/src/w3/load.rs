//! The #486 traffic recipe, under the caller's existing router/air/board lease.
use super::{Echo, stream::Capture};
use crate::{
    router::quote,
    ssh::{Remote, Result, Ssh},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Plan {
    pub count: u32,
    pub interval_ms: u32,
    pub payload_bytes: u32,
    pub recovery_seconds: [u64; 5],
}
impl Plan {
    pub fn select(image: &str, enabled: bool) -> Result<Option<Self>> {
        if !enabled {
            return Ok(None);
        }
        if image != "w3" {
            return Err("--load-1ms requires w3".into());
        }
        Ok(Some(Self {
            count: 10000,
            interval_ms: 1,
            payload_bytes: 56,
            recovery_seconds: [0, 1, 5, 15, 30],
        }))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Batch {
    pub sent: u32,
    pub received: u32,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub plan: Plan,
    pub warmup: Vec<Batch>,
    pub flood: Batch,
    pub recovery: Vec<Batch>,
    pub ap_requests: usize,
    pub ap_replies: usize,
    pub flood_ap_requests: usize,
    pub flood_ap_replies: usize,
    pub capture_complete: bool,
}
impl Report {
    pub fn passed(&self) -> bool {
        self.capture_complete
            && self.flood.sent == self.plan.count
            && self.flood.received >= 9990
            && self.flood_ap_requests == 10000
            && self.flood_ap_replies == self.flood.received as usize
            && self.recovery.len() == 5
            && self.recovery.iter().all(|b| {
                *b == Batch {
                    sent: 5,
                    received: 5,
                }
            })
    }
    pub fn observe(
        &mut self,
        echoes: &[Echo],
        station: Ipv4Addr,
        router: Ipv4Addr,
        complete: bool,
    ) -> Result<()> {
        let mut requests = BTreeSet::new();
        let mut replies = BTreeSet::new();
        for e in echoes {
            if e.source == router && e.destination == station && !e.reply {
                if !requests.insert((e.identifier, e.sequence)) {
                    return Err("duplicate AP load request".into());
                }
            } else if e.source == station
                && e.destination == router
                && e.reply
                && !replies.insert((e.identifier, e.sequence))
            {
                return Err("duplicate AP load reply".into());
            }
        }
        if !replies.is_subset(&requests) {
            return Err("AP load reply without request".into());
        }
        let ids: BTreeSet<_> = requests.iter().map(|(id, _)| *id).collect();
        let floods: Vec<_> = ids
            .into_iter()
            .filter(|id| requests.iter().filter(|(i, _)| i == id).count() > 5)
            .collect();
        let [id] = floods.as_slice() else {
            return Err("missing or ambiguous AP flood identity".into());
        };
        self.ap_requests = requests.len();
        self.ap_replies = replies.len();
        self.flood_ap_requests = requests.iter().filter(|(i, _)| i == id).count();
        self.flood_ap_replies = replies.iter().filter(|(i, _)| i == id).count();
        self.capture_complete = complete;
        Ok(())
    }
}

/// Owns only the temporary, hash-checked fping 5.3 extraction, never an install.
/// SOURCE: #486 p1-stack-fix fixture, OpenWrt 25.12.5 aarch64_cortex-a53.
pub struct Source {
    ssh: Ssh,
    directory: String,
}
impl Source {
    pub fn prepare(ssh: &Ssh) -> Result<Self> {
        let script = r#"set -eu
work=$(mktemp -d /tmp/oer-w3-load.XXXXXX)
trap 'rm -rf "$work"' EXIT HUP INT TERM
mkdir "$work/cache" "$work/packages" "$work/root"
apk --repositories-file /dev/null --repository https://downloads.openwrt.org/releases/25.12.5/packages/aarch64_cortex-a53/base/packages.adb --repository https://downloads.openwrt.org/releases/25.12.5/packages/aarch64_cortex-a53/packages/packages.adb --cache-dir "$work/cache" --no-logfile --timeout 10 update > "$work/prepare.log" 2>&1
apk --repositories-file /dev/null --repository https://downloads.openwrt.org/releases/25.12.5/packages/aarch64_cortex-a53/base/packages.adb --repository https://downloads.openwrt.org/releases/25.12.5/packages/aarch64_cortex-a53/packages/packages.adb --cache-dir "$work/cache" --no-logfile --timeout 10 fetch --output "$work/packages" fping libcap >> "$work/prepare.log" 2>&1
(cd "$work/packages"; sha256sum -c <<'HASHES'
f082d1cff4e392310982b9d904c3906e8d30585776831c8418ed202be599c650  fping-5.3-r1.apk
73b997ced8352bc65466c3de1a106cba1cf97150b0c1dcd6c3df9b6c25393c1b  libcap-2.69-r1.apk
HASHES
) >> "$work/prepare.log" 2>&1
for archive in "$work/packages/fping-5.3-r1.apk" "$work/packages/libcap-2.69-r1.apk"; do
    apk --allow-untrusted extract --destination "$work/root" "$archive" >> "$work/prepare.log" 2>&1
done
export LD_LIBRARY_PATH="$work/root/usr/lib:$work/root/lib"
"$work/root/usr/bin/fping" -v > "$work/version.txt"
grep -q '5.3' "$work/version.txt"
trap - EXIT HUP INT TERM
printf '%s\n' "$work"
"#;
        let output = ssh.output(script, Duration::from_secs(90))?;
        if !output.status.success() {
            return Err("cannot prepare the pinned W3 fping source".into());
        }
        let directory = std::str::from_utf8(&output.stdout)
            .map_err(|_| "invalid W3 source path")?
            .trim()
            .to_owned();
        if !directory.starts_with("/tmp/oer-w3-load.")
            || directory
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"/.-".contains(&b))
        {
            return Err("invalid W3 source path".into());
        }
        Ok(Self {
            ssh: ssh.parallel(),
            directory,
        })
    }
    pub fn capture(
        &self,
        interface: &str,
        address: Ipv4Addr,
        saved: std::fs::File,
        status_file: std::fs::File,
    ) -> Result<Capture> {
        Capture::start_load(
            &self.ssh,
            interface,
            address,
            &self.directory,
            saved,
            status_file,
        )
    }
    fn batch(&self, address: Ipv4Addr, count: u32, interval: u32, payload: u32) -> Result<Batch> {
        if !super::usable_address(address) {
            return Err("unusable load address".into());
        }
        let program = quote(&format!("{}/root/usr/bin/fping", self.directory))?;
        let libraries = quote(&format!("{0}/root/usr/lib:{0}/root/lib", self.directory))?;
        let script = format!(
            "set +e\nLD_LIBRARY_PATH={libraries} {program} -q -c {count} -p {interval} -i 0 -b {payload} -t 2000 {address} 2>&1\nstatus=$?\ncase $status in 0|1) exit 0;; *) exit 1;; esac\n"
        );
        let output = self.ssh.output(&script, Duration::from_secs(35))?;
        if !output.status.success() {
            return Err("W3 load source failed".into());
        }
        let batch = parse_batch(
            std::str::from_utf8(&output.stdout).map_err(|_| "invalid load summary encoding")?,
        )?;
        if batch.sent != count {
            return Err("W3 load source sent a different count".into());
        }
        Ok(batch)
    }
    pub fn run(&self, address: Ipv4Addr, plan: Plan) -> Result<Report> {
        let mut warmup = Vec::new();
        let mut consecutive = 0;
        for _ in 0..20 {
            let batch = self.batch(address, 5, 100, 56)?;
            consecutive = if batch.received == 5 {
                consecutive + 1
            } else {
                0
            };
            warmup.push(batch);
            if consecutive == 2 {
                break;
            }
        }
        if consecutive != 2 {
            return Err("W3 load readiness did not reach two 5/5 batches".into());
        }
        let flood = self.batch(address, plan.count, plan.interval_ms, plan.payload_bytes)?;
        let origin = Instant::now();
        let mut recovery = Vec::new();
        for delay in plan.recovery_seconds {
            oer_os_process::sleep(Duration::from_secs(delay).saturating_sub(origin.elapsed()))
                .map_err(|e| e.to_string())?;
            recovery.push(self.batch(address, 5, 100, 56)?);
        }
        Ok(Report {
            plan,
            warmup,
            flood,
            recovery,
            ap_requests: 0,
            ap_replies: 0,
            flood_ap_requests: 0,
            flood_ap_replies: 0,
            capture_complete: false,
        })
    }
    pub fn finish(mut self) -> Result<()> {
        self.cleanup()?;
        self.directory.clear();
        Ok(())
    }
    fn cleanup(&mut self) -> Result<()> {
        self.ssh
            .run(&format!("rm -rf {}", quote(&self.directory)?))
            .map(|_| ())
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        if !self.directory.is_empty() {
            let _ = self.cleanup();
        }
    }
}

/// Decode a saved load capture. tcpdump emits an empty separator on SIGINT;
/// only this load stream accepts it. Every nonempty row remains mandatory.
pub fn parse_capture(text: &str) -> Result<Vec<Echo>> {
    super::parse_echoes_bounded(text, 4 * 1024 * 1024, 21000, true)
}

pub fn parse_batch(text: &str) -> Result<Batch> {
    let summaries: Vec<_> = text
        .lines()
        .filter_map(|line| line.split_once("xmt/rcv/%loss = ").map(|(_, tail)| tail))
        .collect();
    let [summary] = summaries.as_slice() else {
        return Err("missing or repeated fping summary".into());
    };
    let counts = summary.split(',').next().ok_or("invalid fping summary")?;
    let mut parts = counts.split('/');
    let sent = parts
        .next()
        .ok_or("missing sent count")?
        .parse::<u32>()
        .map_err(|_| "invalid sent count")?;
    let received = parts
        .next()
        .ok_or("missing received count")?
        .parse::<u32>()
        .map_err(|_| "invalid received count")?;
    if parts.next().is_none() || parts.next().is_some() || received > sent {
        return Err("contradictory fping counts".into());
    }
    Ok(Batch { sent, received })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_is_w3_only_and_keeps_the_486_traffic_recipe() {
        assert!(Plan::select("w0", true).is_err());
        assert_eq!(Plan::select("w3", false).unwrap(), None);
        let plan = Plan::select("w3", true).unwrap().unwrap();
        assert_eq!(
            (plan.count, plan.interval_ms, plan.payload_bytes),
            (10000, 1, 56)
        );
        assert_eq!(plan.recovery_seconds, [0, 1, 5, 15, 30]);
    }
    #[test]
    fn ap_capture_separates_flood_from_recovery_and_rejects_duplicates() {
        let station: Ipv4Addr = "192.0.2.8".parse().unwrap();
        let router: Ipv4Addr = "192.0.2.1".parse().unwrap();
        let plan = Plan::select("w3", true).unwrap().unwrap();
        let mut report = Report {
            plan,
            warmup: vec![],
            flood: Batch {
                sent: 10000,
                received: 3043,
            },
            recovery: vec![],
            ap_requests: 0,
            ap_replies: 0,
            flood_ap_requests: 0,
            flood_ap_replies: 0,
            capture_complete: false,
        };
        let mut echoes = Vec::new();
        for (id, count, received) in [(7, 10000, 3043), (8, 5, 5)] {
            for sequence in 0..count {
                echoes.push(Echo {
                    at_us: u64::from(sequence),
                    source: router,
                    destination: station,
                    reply: false,
                    identifier: id,
                    sequence,
                });
                if sequence < received {
                    echoes.push(Echo {
                        at_us: u64::from(sequence) + 1,
                        source: station,
                        destination: router,
                        reply: true,
                        identifier: id,
                        sequence,
                    });
                }
            }
        }
        report.observe(&echoes, station, router, true).unwrap();
        assert_eq!((report.ap_requests, report.ap_replies), (10005, 3048));
        assert_eq!(
            (report.flood_ap_requests, report.flood_ap_replies),
            (10000, 3043)
        );
        assert!(!report.passed());
        echoes.push(echoes[0].clone());
        assert!(report.observe(&echoes, station, router, true).is_err());
    }

    #[test]
    fn summaries_preserve_loss_and_refuse_missing_repeated_or_impossible_counts() {
        assert_eq!(
            parse_batch("192.0.2.8 : xmt/rcv/%loss = 10000/3043/70%, min/avg/max = 1/2/3").unwrap(),
            Batch {
                sent: 10000,
                received: 3043
            }
        );
        for text in [
            "",
            "xmt/rcv/%loss = 1/2/0%",
            "xmt/rcv/%loss = 1/1/0%\nxmt/rcv/%loss = 1/1/0%",
        ] {
            assert!(parse_batch(text).is_err());
        }
    }
}
