//! Session-bounded, read-only evidence from the laboratory OpenWrt AP.

use crate::capture_process;
use oer_process::CommandExt as _;
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    time::Duration,
};

use crate::Result;
use oer_hil_lab::config::OpenWrtConfig;
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};

const PRE_WORKLOAD_CHANNEL_SAMPLE: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ChannelUtilization {
    pub active_millis: u64,
    pub busy_millis: u64,
    pub scaled_255: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OpenWrtRxEvidence {
    pub ingress_packets: u64,
    pub wireless_packets: u64,
    pub ingress_interface_rx_packets: u64,
    pub wireless_interface_tx_packets: u64,
    pub station_tx_packets: u64,
    pub station_tx_retries: u64,
    pub station_tx_failed: u64,
    pub station_tx_duration_micros: u64,
    pub station_tid0_aqm_drops: u64,
    pub pre_workload_channel_utilization: Option<ChannelUtilization>,
    pub workload_channel_utilization: ChannelUtilization,
    pub channel_width_mhz: u8,
    pub tx_bitrate: String,
    pub rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    station_mac: [u8; 6],
    ingress_rx_packets: u64,
    wireless_tx_packets: u64,
    station_tx_packets: u64,
    station_tx_retries: u64,
    station_tx_failed: u64,
    station_tx_duration_micros: u64,
    station_tid0_aqm_drops: u64,
    channel_active_millis: u64,
    channel_busy_millis: u64,
    channel_width_mhz: u8,
    tx_bitrate: String,
    rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenWrtStationLinkEvidence {
    pub channel_width_mhz: u8,
    pub tx_bitrate: String,
    pub rx_bitrate: String,
}

struct Capture {
    name: &'static str,
    child: capture_process::Capture,
}

struct OpenWrtRateMask {
    config: OpenWrtConfig,
    active: bool,
}

impl OpenWrtRateMask {
    fn apply(
        config: &OpenWrtConfig,
        forced_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Option<Self>> {
        let interval = match forced_guard_interval {
            HtGuardIntervalExpectation::Any => return Ok(None),
            HtGuardIntervalExpectation::Long => "lgi-2.4",
            HtGuardIntervalExpectation::Short => "sgi-2.4",
        };
        if config.read_only {
            return Err("read-only OpenWrt fixture forbids rate changes".into());
        }
        let script = format!(
            "set -eu; iw dev {} set bitrates {interval}",
            config.wireless_interface
        );
        let owner = Self {
            config: config.clone(),
            active: true,
        };
        let output = openwrt_command(config, &script)?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "cannot apply OpenWrt HT {} guard-interval policy: {}",
                forced_guard_interval.id(),
                String::from_utf8_lossy(&output.stderr).trim(),
            ))
            .into());
        }
        Ok(Some(owner))
    }

    fn clear(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        let script = format!(
            "set -eu; iw dev {} set bitrates",
            self.config.wireless_interface
        );
        let output = openwrt_command(&self.config, &script)?;
        if !output.status.success() {
            return Err(crate::Error::new(format!(
                "cannot restore the OpenWrt automatic rate mask: {}",
                String::from_utf8_lossy(&output.stderr).trim(),
            ))
            .into());
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for OpenWrtRateMask {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            oer_hil_workload::fixture::cleanup::record("restore automatic rate mask", || {
                self.clear()
            });
        });
    }
}

pub struct OpenWrtRxCapture {
    config: OpenWrtConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    before: Snapshot,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    ingress: Option<Capture>,
    wireless: Option<crate::openwrt::capture::RemoteCapture>,
    rate_mask: Option<OpenWrtRateMask>,
}

impl OpenWrtRxCapture {
    pub fn start(
        config: &OpenWrtConfig,
        endpoint: SocketAddrV4,
        output: &Path,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
        forced_guard_interval: HtGuardIntervalExpectation,
        maximum_idle_channel_utilization_255: Option<u8>,
    ) -> Result<Self> {
        let target = *endpoint.ip();
        let port = endpoint.port();
        if forced_guard_interval != HtGuardIntervalExpectation::Any
            && expected_phy != PhyExpectation::Ht40
        {
            return Err(crate::Error::new(
                "OpenWrt guard-interval control currently owns only the HT40 MCS7 diagnostic",
            )
            .into());
        }
        let rate_mask = OpenWrtRateMask::apply(config, forced_guard_interval)?;
        let pre_workload_channel_utilization = maximum_idle_channel_utilization_255
            .map(|maximum| -> Result<_> {
                let utilization =
                    measure_channel_utilization(config).map_err(crate::Error::context)?;
                require_pre_workload_channel_utilization(Some(maximum), utilization.scaled_255)?;
                Ok(utilization)
            })
            .transpose()?;
        let before = snapshot(config, target, None).map_err(crate::Error::context)?;
        require_width(expected_phy, before.channel_width_mhz)?;
        let timeout = traffic_duration.saturating_add(Duration::from_secs(3));
        let ingress = spawn_capture(
            config,
            "OpenWrt Ethernet ingress",
            &config.ingress_interface,
            target,
            port,
            timeout,
        )?;
        let wireless = crate::openwrt::capture::RemoteCapture::start_managed(
            config,
            &config.wireless_interface,
            output.join("openwrt-wifi-egress.pcap"),
            &format!("udp and dst host {target} and dst port {port}"),
            timeout,
        )?;
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            before,
            pre_workload_channel_utilization,
            ingress: Some(ingress),
            wireless: Some(wireless),
            rate_mask,
        })
    }

    pub fn finish(mut self) -> Result<OpenWrtRxEvidence> {
        let ingress = finish_capture(self.ingress.take().expect("capture owns ingress"))
            .map_err(crate::Error::context)?;
        let mut wireless_capture = self.wireless.take().expect("capture owns wireless");
        let (wireless, _) = wireless_capture
            .finish_capture()
            .map_err(crate::Error::context)?;
        drop(wireless_capture);
        let after = snapshot(&self.config, self.target, Some(self.before.station_mac))
            .map_err(crate::Error::context)?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
        if let Some(rate_mask) = self.rate_mask.as_mut() {
            rate_mask.clear()?;
        }
        let workload_channel_active_millis = delta(
            "workload channel active time",
            self.before.channel_active_millis,
            after.channel_active_millis,
        )?;
        let workload_channel_busy_millis = delta(
            "workload channel busy time",
            self.before.channel_busy_millis,
            after.channel_busy_millis,
        )?;
        Ok(OpenWrtRxEvidence {
            ingress_packets: ingress,
            wireless_packets: wireless,
            ingress_interface_rx_packets: delta(
                "ingress interface RX",
                self.before.ingress_rx_packets,
                after.ingress_rx_packets,
            )?,
            wireless_interface_tx_packets: delta(
                "wireless interface TX",
                self.before.wireless_tx_packets,
                after.wireless_tx_packets,
            )?,
            station_tx_packets: delta(
                "station TX",
                self.before.station_tx_packets,
                after.station_tx_packets,
            )?,
            station_tx_retries: delta(
                "station retries",
                self.before.station_tx_retries,
                after.station_tx_retries,
            )?,
            station_tx_failed: delta(
                "station failed",
                self.before.station_tx_failed,
                after.station_tx_failed,
            )?,
            station_tx_duration_micros: delta(
                "station TX duration",
                self.before.station_tx_duration_micros,
                after.station_tx_duration_micros,
            )?,
            station_tid0_aqm_drops: delta(
                "station TID-0 AQM drops",
                self.before.station_tid0_aqm_drops,
                after.station_tid0_aqm_drops,
            )?,
            pre_workload_channel_utilization: self.pre_workload_channel_utilization,
            workload_channel_utilization: ChannelUtilization {
                active_millis: workload_channel_active_millis,
                busy_millis: workload_channel_busy_millis,
                scaled_255: scale_channel_utilization(
                    workload_channel_active_millis,
                    workload_channel_busy_millis,
                )?,
            },
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

fn openwrt_command(config: &OpenWrtConfig, script: &str) -> Result<std::process::Output> {
    oer_stand_ssh::command(&config.ssh_target, script)
        .supervised_output()
        .and_then(crate::Error::ssh_output)
}

/// Snapshot the final AP/STA link vector after one measured workload.
pub fn station_link(
    config: &OpenWrtConfig,
    target: Ipv4Addr,
) -> Result<OpenWrtStationLinkEvidence> {
    let snapshot = snapshot(config, target, None).map_err(crate::Error::context)?;
    Ok(OpenWrtStationLinkEvidence {
        channel_width_mhz: snapshot.channel_width_mhz,
        tx_bitrate: snapshot.tx_bitrate,
        rx_bitrate: snapshot.rx_bitrate,
    })
}

fn snapshot(
    config: &OpenWrtConfig,
    target: Ipv4Addr,
    station_mac: Option<[u8; 6]>,
) -> Result<Snapshot> {
    let mac_assignment = match station_mac {
        Some(mac) => format!(
            "mac='{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}'; ",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ),
        None => format!(
            "ping -c 1 -W 1 {target} >/dev/null 2>&1 || true; \
             mac=$(ip neigh show {target} | awk '{{for (i = 1; i < NF; i++) if ($i == \"lladdr\") {{print $(i + 1); exit}}}}'); \
             if test -z \"$mac\"; then \
               set -- $(iw dev {} station dump | awk '/^Station / {{print $2}}'); \
               test \"$#\" -eq 1; mac=\"$1\"; \
             fi; ",
            config.wireless_interface
        ),
    };
    let script = format!(
        "set -eu; \
         {mac_assignment}\
         test -n \"$mac\"; \
         stats=$(iw dev {wireless} station get \"$mac\"); \
         airtime=$(ubus -S call hostapd.{wireless} get_status); \
         printf 'station_mac=%s\n' \"$mac\"; \
         printf 'ingress_rx=%s\\n' \"$(cat /sys/class/net/{ingress}/statistics/rx_packets)\"; \
         printf 'wireless_tx=%s\\n' \"$(cat /sys/class/net/{wireless}/statistics/tx_packets)\"; \
         printf 'station_tx=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx packets:/ {{print $3}}')\"; \
         printf 'station_retries=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx retries:/ {{print $3}}')\"; \
         printf 'station_failed=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx failed:/ {{print $3}}')\"; \
         printf 'station_tx_duration_us=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx duration:/ {{print $3}}')\"; \
         printf 'channel_active_ms=%s\\n' \"$(jsonfilter -s \"$airtime\" -e '@.airtime.time')\"; \
         printf 'channel_busy_ms=%s\\n' \"$(jsonfilter -s \"$airtime\" -e '@.airtime.time_busy')\"; \
         printf 'tx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*tx bitrate:[[:space:]]*//p')\"; \
         printf 'rx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*rx bitrate:[[:space:]]*//p')\"; \
         set -- /sys/kernel/debug/ieee80211/*/netdev:{wireless}/stations/$mac/aqm; \
         test \"$#\" -eq 1; test -r \"$1\"; \
         printf 'station_tid0_aqm_drops=%s\\n' \"$(awk '$1 == 0 {{print $6}}' \"$1\")\"; \
         printf 'channel_width=%s\\n' \"$(iw dev {wireless} info | sed -n 's/.*width: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"",
        ingress = config.ingress_interface,
        wireless = config.wireless_interface,
    );
    let output = oer_stand_ssh::command(&config.ssh_target, &script).supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot snapshot OpenWrt station counters: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(Snapshot {
        station_mac: parse_mac(tagged_text(&stdout, "station_mac")?)?,
        ingress_rx_packets: tagged(&stdout, "ingress_rx")?,
        wireless_tx_packets: tagged(&stdout, "wireless_tx")?,
        station_tx_packets: tagged(&stdout, "station_tx")?,
        station_tx_retries: tagged(&stdout, "station_retries")?,
        station_tx_failed: tagged(&stdout, "station_failed")?,
        station_tx_duration_micros: tagged(&stdout, "station_tx_duration_us")?,
        station_tid0_aqm_drops: tagged(&stdout, "station_tid0_aqm_drops")?,
        channel_active_millis: tagged(&stdout, "channel_active_ms")?,
        channel_busy_millis: tagged(&stdout, "channel_busy_ms")?,
        channel_width_mhz: u8::try_from(tagged(&stdout, "channel_width")?)?,
        tx_bitrate: nonempty_tagged_text(&stdout, "tx_bitrate")?.to_owned(),
        rx_bitrate: nonempty_tagged_text(&stdout, "rx_bitrate")?.to_owned(),
    })
}

fn tagged_text<'a>(output: &'a str, key: &str) -> Result<&'a str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt snapshot omitted `{key}`").into())
}

fn nonempty_tagged_text<'a>(output: &'a str, key: &str) -> Result<&'a str> {
    tagged_text(output, key).and_then(|value| {
        (!value.is_empty())
            .then_some(value)
            .ok_or_else(|| format!("OpenWrt snapshot reported an empty `{key}`").into())
    })
}

fn parse_mac(value: &str) -> Result<[u8; 6]> {
    let mut bytes = [0_u8; 6];
    let mut octets = value.split(':');
    for byte in &mut bytes {
        let octet = octets
            .next()
            .filter(|octet| octet.len() == 2)
            .ok_or_else(|| format!("invalid OpenWrt station MAC `{value}`"))?;
        *byte = u8::from_str_radix(octet, 16)
            .map_err(|_| format!("invalid OpenWrt station MAC `{value}`"))?;
    }
    if octets.next().is_some() {
        return Err(crate::Error::new(format!("invalid OpenWrt station MAC `{value}`")).into());
    }
    Ok(bytes)
}

pub fn resolve_station_mac(config: &OpenWrtConfig, target: Ipv4Addr) -> Result<String> {
    let script = format!(
        "set -eu; \
         ping -c 1 -W 1 {target} >/dev/null 2>&1 || true; \
         mac=$(ip neigh show {target} | awk '{{for (i = 1; i < NF; i++) if ($i == \"lladdr\") {{print $(i + 1); exit}}}}'); \
         if test -z \"$mac\"; then \
           set -- $(iw dev {} station dump | awk '/^Station / {{print $2}}'); \
           test \"$#\" -eq 1; mac=\"$1\"; \
         fi; \
         printf '%s\\n' \"$mac\"",
        config.wireless_interface
    );
    let output = oer_stand_ssh::command(&config.ssh_target, &script).supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot resolve the sole associated OpenWrt station: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let mac = String::from_utf8(output.stdout)?
        .trim()
        .to_ascii_lowercase();
    parse_mac(&mac)?;
    Ok(mac)
}

fn require_width(phy: PhyExpectation, observed: u8) -> Result<()> {
    let expected = match phy {
        PhyExpectation::Legacy | PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
        PhyExpectation::Ht40 => 40,
    };
    if observed != expected {
        return Err(crate::Error::new(format!(
            "OpenWrt AP link width changed during the HIL session: expected={expected} observed={observed} MHz"
        )).into());
    }
    Ok(())
}

fn require_pre_workload_channel_utilization(maximum: Option<u8>, observed: u8) -> Result<()> {
    if maximum.is_some_and(|maximum| observed > maximum) {
        return Err(crate::Error::new(format!(
            "OpenWrt pre-workload channel utilization is too high for a ceiling run: observed={observed}/255 maximum={}/255",
            maximum.expect("checked above"),
        )).into());
    }
    Ok(())
}

pub fn require_idle_channel_utilization(
    config: &OpenWrtConfig,
    maximum: u8,
) -> Result<ChannelUtilization> {
    let utilization = measure_channel_utilization(config).map_err(crate::Error::context)?;
    require_pre_workload_channel_utilization(Some(maximum), utilization.scaled_255)?;
    Ok(utilization)
}

fn measure_channel_utilization(config: &OpenWrtConfig) -> Result<ChannelUtilization> {
    let script = format!(
        "set -eu; \
         before=$(ubus -S call hostapd.{wireless} get_status); \
         active_before=$(jsonfilter -s \"$before\" -e '@.airtime.time'); \
         busy_before=$(jsonfilter -s \"$before\" -e '@.airtime.time_busy'); \
         sleep {seconds}; \
         after=$(ubus -S call hostapd.{wireless} get_status); \
         printf 'active_before=%s\\n' \"$active_before\"; \
         printf 'busy_before=%s\\n' \"$busy_before\"; \
         printf 'active_after=%s\\n' \"$(jsonfilter -s \"$after\" -e '@.airtime.time')\"; \
         printf 'busy_after=%s\\n' \"$(jsonfilter -s \"$after\" -e '@.airtime.time_busy')\"",
        wireless = config.wireless_interface,
        seconds = PRE_WORKLOAD_CHANNEL_SAMPLE.as_secs(),
    );
    let output = oer_stand_ssh::command(&config.ssh_target, &script).supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot measure OpenWrt pre-workload channel utilization: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    let active_millis = delta(
        "channel active time",
        tagged(&stdout, "active_before")?,
        tagged(&stdout, "active_after")?,
    )?;
    let busy_millis = delta(
        "channel busy time",
        tagged(&stdout, "busy_before")?,
        tagged(&stdout, "busy_after")?,
    )?;
    let scaled_255 = scale_channel_utilization(active_millis, busy_millis)?;
    Ok(ChannelUtilization {
        active_millis,
        busy_millis,
        scaled_255,
    })
}

fn scale_channel_utilization(active_millis: u64, busy_millis: u64) -> Result<u8> {
    if active_millis == 0 || busy_millis > active_millis {
        return Err(crate::Error::new(format!(
            "invalid OpenWrt channel survey delta: active={active_millis} ms busy={busy_millis} ms"
        ))
        .into());
    }
    let scaled = (u128::from(busy_millis) * 255).div_ceil(u128::from(active_millis));
    Ok(u8::try_from(scaled)?)
}

fn tagged(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt snapshot omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
}

fn delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!("OpenWrt `{name}` counter reset during the HIL session: {before} -> {after}").into()
    })
}

pub fn doctor_tools(config: &OpenWrtConfig) -> Result<()> {
    let script = format!(
        "set -eu; command -v tcpdump >/dev/null; command -v timeout >/dev/null; \
         command -v ubus >/dev/null; command -v jsonfilter >/dev/null; \
         test -d /sys/kernel/debug/ieee80211; \
         test -d /sys/class/net/{ingress}",
        ingress = config.ingress_interface,
    );
    let output = oer_stand_ssh::command(&config.ssh_target, &script).supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "OpenWrt fixture doctor failed over noninteractive SSH: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    Ok(())
}

fn spawn_capture(
    config: &OpenWrtConfig,
    name: &'static str,
    interface: &str,
    target: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<Capture> {
    let filter = format!("udp and dst host {target} and dst port {port}");
    let program = format!(
        "tcpdump -i {interface} -n -q -U -w /dev/null {}",
        capture_process::quote(&filter)
    );
    let lifetime = timeout.saturating_add(Duration::from_secs(120));
    let script = format!(
        "LC_ALL=C timeout -s TERM {} sh -c {}",
        lifetime.as_secs(),
        capture_process::quote(&capture_process::controlled(&program, ":"))
    );
    let mut command = oer_stand_ssh::command(&config.ssh_target, &script);
    let child = capture_process::Capture::start(
        &mut command,
        format!("tcpdump: listening on {interface},"),
        lifetime,
    )?;
    Ok(Capture { name, child })
}

fn finish_capture(capture: Capture) -> Result<u64> {
    let output = capture.child.finish()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let captured = parse_summary_value(&stderr, "packets captured")
        .ok_or_else(|| format!("{} did not report a packet count: {stderr}", capture.name))?;
    let dropped = parse_summary_value(&stderr, "packets dropped by kernel")
        .ok_or("tcpdump omitted its drop count")?;
    if dropped != 0 {
        return Err(crate::Error::new(format!(
            "{} dropped {dropped} captured packets",
            capture.name
        ))
        .into());
    }
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "{} exited with {}: {stderr}",
            capture.name, output.status
        ))
        .into());
    }
    Ok(captured)
}

pub fn check_capture(config: &OpenWrtConfig) -> Result<()> {
    let address = Ipv4Addr::new(192, 0, 2, 1);
    let ingress = spawn_capture(
        config,
        "OpenWrt ingress self-check",
        &config.ingress_interface,
        address,
        9,
        Duration::from_secs(1),
    )?;
    let wireless = spawn_capture(
        config,
        "OpenWrt wireless self-check",
        &config.wireless_interface,
        address,
        9,
        Duration::from_secs(1),
    )?;
    finish_capture(ingress)?;
    finish_capture(wireless)?;
    Ok(())
}

fn parse_summary_value(stderr: &str, suffix: &str) -> Option<u64> {
    stderr.lines().find_map(|line| {
        let line = line.trim();
        let number = line.strip_suffix(suffix)?.trim();
        number.parse().ok()
    })
}

#[cfg(test)]
mod tests;
