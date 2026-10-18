//! Scoped controlled AP client on the laboratory OpenWrt radio.

use oer_process::CommandExt as _;
use std::{
    io::Write as _,
    net::Ipv4Addr,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

use zeroize::{Zeroize, Zeroizing};

use oer_hil_lab::wifi::WifiAccessPointSecurity;

use crate::Result;
use oer_hil_lab::config::{AccessPointConfig, OpenWrtConfig};
use oer_hil_scenario_catalog::link::HtGuardIntervalExpectation;

// Linux network-interface names contain at most IFNAMSIZ-1 (15) bytes.
// Keeping the fixture name below that boundary avoids an opaque nl80211
// `Attribute failed policy validation` error before association even starts.
const INTERFACE: &str = "or-ap-client";
// A virtual managed interface otherwise inherits the active AP interface's
// address. mt76 accepts the wdev creation but refuses to bring the duplicate
// address up. This locally administered identity belongs only to the scoped
// laboratory client and disappears together with the interface.
const CLIENT_MAC: &str = "02:00:00:00:43:03";
const PID_FILE: &str = "/var/run/open-radio-client.pid";
const CONFIG_FILE: &str = "/var/run/open-radio-client.conf";
const FORWARD_TABLE: &str = "open_radio_hil";
const FORWARD_CHAIN: &str = "open_radio_hil_forward";
// Only the completed association/liveness wait uses this status. Remote setup
// errors retain their exit status and belong to the laboratory boundary.
const ASSOCIATION_TIMEOUT: i32 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct SecondaryClientProbeEvidence {
    pub transmitted: u32,
    pub received: u32,
}

pub struct ControlledOpenWrtClient {
    fixture: OpenWrtConfig,
    forward_address: Option<Ipv4Addr>,
    restored: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OpenWrtClientFixturePreparation {
    pub wireless_restarted: bool,
    pub elapsed_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OpenWrtClientLinkEvidence {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_duration_micros: Option<u64>,
    pub rx_bitrate: Option<String>,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bitrate: Option<String>,
    pub tx_retries: u64,
    pub tx_failed: u64,
    pub tx_duration_micros: u64,
    pub tid0_aqm_drops: u64,
    /// Frames from the AP that mac80211 dropped after reception.
    pub rx_drop_misc: u64,
    /// Averaged signal of the AP's frames at the client after the workload.
    pub signal_avg_dbm: Option<i32>,
    /// Survey of the operating primary channel during the workload.
    pub channel: OpenWrtClientChannelSurvey,
}

/// Channel time the client radio attributed to each activity during the
/// workload. Busy time outside BSS reception and own transmission is air
/// occupied by other transmitters or non-decodable energy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OpenWrtClientChannelSurvey {
    pub active_millis: u64,
    pub busy_millis: u64,
    pub receive_millis: u64,
    pub bss_receive_millis: u64,
    pub transmit_millis: u64,
    pub noise_dbm: Option<i32>,
}

#[derive(Clone)]
struct OpenWrtClientLinkSnapshot {
    rx_bytes: u64,
    rx_packets: u64,
    rx_duration_micros: Option<u64>,
    rx_bitrate: Option<String>,
    tx_bytes: u64,
    tx_packets: u64,
    tx_bitrate: Option<String>,
    tx_retries: u64,
    tx_failed: u64,
    tx_duration_micros: u64,
    tid0_aqm_drops: u64,
    rx_drop_misc: u64,
    signal_avg_dbm: Option<i32>,
    survey: OpenWrtClientChannelSurvey,
}

pub struct OpenWrtClientLinkObservation {
    fixture: OpenWrtConfig,
    before: OpenWrtClientLinkSnapshot,
}

pub fn doctor(_access_point: &AccessPointConfig, fixture: &OpenWrtConfig) -> Result<()> {
    let status = oer_process::command("sh")
        .args(["-c", "command -v wpa_passphrase >/dev/null"])
        .supervised_status()?;
    if !status.success() {
        return Err("wpa_passphrase is required for the controlled OpenWrt AP client".into());
    }
    let script = format!(
        "set -eu; \
         command -v iw >/dev/null; \
         command -v ip >/dev/null; \
         command -v ping >/dev/null; \
         command -v wpa_supplicant >/dev/null; \
         command -v wifi >/dev/null; \
         command -v nft >/dev/null; \
         command -v fw4 >/dev/null; \
         test \"$(sysctl -n net.ipv4.ip_forward)\" = 1; \
         phy=$(ubus call iwinfo phyname '{{\"section\":\"{}\"}}' | jsonfilter -e '@.phyname'); \
         iw phy \"$phy\" info | grep -q '#{{ managed }}'",
        fixture.radio,
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "OpenWrt fixture cannot host a controlled AP client: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    Ok(())
}

impl ControlledOpenWrtClient {
    /// Prepare the scoped client after the scenario owner has configured and
    /// restarted the selected radio. This owner never resets global wireless.
    pub fn prepare_fixture(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
    ) -> Result<OpenWrtClientFixturePreparation> {
        let started = Instant::now();
        restore(fixture)?;
        let observed = crate::openwrt::ap::observe(fixture)?;
        let width = access_point.bandwidth_mhz();
        if !observed.enabled
            || observed.channel != access_point.channel()
            || !observed.geometry.contains(&format!("width: {width} MHz"))
        {
            return Err(crate::Error::new(format!(
                "prepared OpenWrt AP-client geometry differs from the target AP: {observed:?}"
            ))
            .into());
        }
        Ok(OpenWrtClientFixturePreparation {
            wireless_restarted: false,
            elapsed_millis: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        })
    }

    pub fn connect(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        let address = access_point.secondary_client_cidr().ok_or(
            "the AP scenario requires a second client but secondary_client_address is absent",
        )?;
        let client_address = access_point.secondary_client_address().ok_or(
            "the AP scenario requires a second client but secondary_client_address is absent",
        )?;
        Self::connect_at(
            access_point,
            fixture,
            address,
            client_address,
            security,
            fixed_ht_mcs,
            fixed_guard_interval,
        )
    }

    pub fn connect_primary(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        Self::connect_at(
            access_point,
            fixture,
            access_point.client_cidr(),
            access_point.client_address(),
            security,
            fixed_ht_mcs,
            fixed_guard_interval,
        )
    }

    fn connect_at(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        address: String,
        client_address: Ipv4Addr,
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        if fixed_ht_mcs.is_some_and(|mcs| mcs > 7) {
            return Err("controlled OpenWrt AP-client fixed HT MCS must be within 0..=7".into());
        }
        let (ssid, passphrase) = access_point.credentials();
        let target = access_point.target_address();
        let frequency_mhz = access_point.frequency_mhz();
        let ssid_hex = encode_hex(ssid.as_bytes());
        let security_config = match security {
            WifiAccessPointSecurity::Open => Zeroizing::new(String::from("key_mgmt=NONE")),
            WifiAccessPointSecurity::Wpa2Personal => {
                let psk = derive_psk(ssid, passphrase).map_err(crate::Error::context)?;
                Zeroizing::new(format!(
                    "psk={}\\n  key_mgmt=WPA-PSK\\n  proto=RSN\\n  pairwise=CCMP\\n  group=CCMP",
                    psk.as_str(),
                ))
            }
        };
        let phy = format!(
            "/sys/class/net/{}/phy80211/name",
            fixture.wireless_interface
        );
        let script = Zeroizing::new(format!(
            "set -eu; \
             phy=$(cat {phy}); \
             if test -f {PID_FILE}; then old_pid=$(cat {PID_FILE}); kill \"$old_pid\" 2>/dev/null || true; for attempt in $(seq 1 5); do kill -0 \"$old_pid\" 2>/dev/null || break; sleep 1; done; fi; \
             iw dev {INTERFACE} del 2>/dev/null || true; \
             rm -f {PID_FILE} {CONFIG_FILE}; \
             iw phy \"$phy\" interface add {INTERFACE} type managed addr {CLIENT_MAC}; \
             ip link set {INTERFACE} up; \
             ip addr add {address} dev {INTERFACE}; \
             umask 077; \
             printf 'ctrl_interface=/var/run/wpa_supplicant\\nnetwork={{\\n  ssid={ssid_hex}\\n  {security_config}\\n}}\\n' > {CONFIG_FILE}; \
             sed -i '/^}}$/i\\  scan_freq={frequency_mhz}' {CONFIG_FILE}; \
             wpa_supplicant -B -P {PID_FILE} -i {INTERFACE} -c {CONFIG_FILE}; \
             if command -v wpa_cli >/dev/null; then wpa_cli -i {INTERFACE} scan >/dev/null; fi; \
             for attempt in $(seq 1 20); do \
                 if ping -4 -q -I {INTERFACE} -c 1 -W 1 {target} >/dev/null 2>&1; then \
                     exit 0; \
                 fi; \
                 sleep 1; \
             done; \
             iw dev {INTERFACE} link >&2 || true; \
             if command -v wpa_cli >/dev/null; then wpa_cli -i {INTERFACE} status >&2 || true; fi; \
             exit {ASSOCIATION_TIMEOUT}",
            security_config = security_config.as_str(),
        ));
        // Own rollback before the first remote mutation, including partial setup.
        let mut owner = Self {
            fixture: fixture.clone(),
            forward_address: None,
            restored: false,
        };
        let output = ssh(fixture, script.as_str())?;
        require_association(output)?;
        if fixed_ht_mcs.is_some() || fixed_guard_interval != HtGuardIntervalExpectation::Any {
            let mut rate_mask = format!("iw dev {INTERFACE} set bitrates");
            if let Some(mcs) = fixed_ht_mcs {
                rate_mask.push_str(&format!(" ht-mcs-2.4 {mcs}"));
            }
            match fixed_guard_interval {
                HtGuardIntervalExpectation::Any => {}
                HtGuardIntervalExpectation::Long => rate_mask.push_str(" lgi-2.4"),
                HtGuardIntervalExpectation::Short => rate_mask.push_str(" sgi-2.4"),
            }
            let output = ssh(fixture, &rate_mask)?;
            if !output.status.success() {
                return Err(crate::Error::new(format!(
                    "cannot apply the controlled OpenWrt AP-client HT rate mask `{rate_mask}`: {}",
                    String::from_utf8_lossy(&output.stderr).trim(),
                ))
                .into());
            }
        }
        // Association is visible before all bridge/driver counters settle.
        // Keep this outside the measured workload and give the target time to
        // publish its controlled-port state.
        oer_process::sleep(Duration::from_millis(250))?;
        owner.forward_address = Some(
            install_forwarding(fixture, access_point.target_address(), client_address)
                .map_err(crate::Error::context)?,
        );
        Ok(owner)
    }

    /// Address reached by the wired host while OpenWrt owns the Wi-Fi peer.
    ///
    /// The scoped forwarding rules translate this management address to the
    /// DUT AP address. The host therefore remains the traffic generator, but
    /// every measured radio frame still crosses the controlled OpenWrt peer.
    pub const fn forward_address(&self) -> Option<Ipv4Addr> {
        self.forward_address
    }

    pub fn begin_link_observation(&self) -> Result<OpenWrtClientLinkObservation> {
        Ok(OpenWrtClientLinkObservation {
            fixture: self.fixture.clone(),
            before: snapshot_link(&self.fixture).map_err(crate::Error::context)?,
        })
    }

    pub fn restore(mut self) -> Result<()> {
        restore(&self.fixture)?;
        self.restored = true;
        Ok(())
    }

    /// Exercise this peer throughout the primary saturation workload.
    ///
    /// The probe is deliberately low-bandwidth: it proves that AP scheduling,
    /// pairwise key selection and the network handoff continue to serve a
    /// second peer without contaminating the primary throughput measurement.
    pub fn spawn_probe(
        &self,
        target: Ipv4Addr,
        duration: Duration,
    ) -> thread::JoinHandle<Result<SecondaryClientProbeEvidence>> {
        let fixture = self.fixture.clone();
        thread::spawn(move || probe(&fixture, target, duration))
    }
}

fn require_association(output: std::process::Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let message = format!(
        "OpenWrt AP client connection failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim(),
    );
    if output.status.code() == Some(ASSOCIATION_TIMEOUT) {
        Err(message.into())
    } else {
        Err(crate::Error::new(message).into())
    }
}

impl OpenWrtClientLinkObservation {
    pub fn finish(self) -> Result<OpenWrtClientLinkEvidence> {
        let after = snapshot_link(&self.fixture).map_err(crate::Error::context)?;
        Ok(OpenWrtClientLinkEvidence {
            rx_bytes: counter_delta("RX bytes", self.before.rx_bytes, after.rx_bytes)?,
            rx_packets: counter_delta("RX packets", self.before.rx_packets, after.rx_packets)?,
            rx_duration_micros: optional_counter_delta(
                "RX duration",
                self.before.rx_duration_micros,
                after.rx_duration_micros,
            )?,
            rx_bitrate: after.rx_bitrate,
            tx_bytes: counter_delta("TX bytes", self.before.tx_bytes, after.tx_bytes)?,
            tx_packets: counter_delta("TX packets", self.before.tx_packets, after.tx_packets)?,
            tx_bitrate: after.tx_bitrate,
            tx_retries: counter_delta("TX retries", self.before.tx_retries, after.tx_retries)?,
            tx_failed: counter_delta("TX failed", self.before.tx_failed, after.tx_failed)?,
            tx_duration_micros: counter_delta(
                "TX duration",
                self.before.tx_duration_micros,
                after.tx_duration_micros,
            )?,
            tid0_aqm_drops: counter_delta(
                "TID-0 AQM drops",
                self.before.tid0_aqm_drops,
                after.tid0_aqm_drops,
            )?,
            rx_drop_misc: counter_delta(
                "RX drop misc",
                self.before.rx_drop_misc,
                after.rx_drop_misc,
            )?,
            signal_avg_dbm: after.signal_avg_dbm,
            channel: OpenWrtClientChannelSurvey {
                active_millis: counter_delta(
                    "channel active time",
                    self.before.survey.active_millis,
                    after.survey.active_millis,
                )?,
                busy_millis: counter_delta(
                    "channel busy time",
                    self.before.survey.busy_millis,
                    after.survey.busy_millis,
                )?,
                receive_millis: counter_delta(
                    "channel receive time",
                    self.before.survey.receive_millis,
                    after.survey.receive_millis,
                )?,
                bss_receive_millis: counter_delta(
                    "channel BSS receive time",
                    self.before.survey.bss_receive_millis,
                    after.survey.bss_receive_millis,
                )?,
                transmit_millis: counter_delta(
                    "channel transmit time",
                    self.before.survey.transmit_millis,
                    after.survey.transmit_millis,
                )?,
                noise_dbm: after.survey.noise_dbm,
            },
        })
    }
}

impl Drop for ControlledOpenWrtClient {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            if !self.restored {
                oer_hil_workload::fixture::cleanup::record("restore OpenWrt client", || {
                    restore(&self.fixture)
                });
            }
        });
    }
}

fn install_forwarding(
    fixture: &OpenWrtConfig,
    target: Ipv4Addr,
    client: Ipv4Addr,
) -> Result<Ipv4Addr> {
    let cleanup = cleanup_forwarding_script();
    let script = format!(
        "set -eu; \
         cleanup_forwarding() {{ {cleanup}; }}; \
         cleanup_forwarding; \
         trap 'cleanup_forwarding' EXIT; \
         host_ip=${{SSH_CLIENT%% *}}; \
         route=$(ip -4 route get \"$host_ip\"); \
         host_if=$(printf '%s\\n' \"$route\" | awk '{{for (i=1; i<=NF; i++) if ($i == \"dev\") {{print $(i+1); exit}}}}'); \
         management_ip=$(printf '%s\\n' \"$route\" | awk '{{for (i=1; i<=NF; i++) if ($i == \"src\") {{print $(i+1); exit}}}}'); \
         test -n \"$host_if\"; \
         test -n \"$management_ip\"; \
         nft add chain inet fw4 {FORWARD_CHAIN}; \
         nft insert rule inet fw4 forward jump {FORWARD_CHAIN}; \
         nft add rule inet fw4 {FORWARD_CHAIN} iifname \"$host_if\" oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} accept; \
         nft add rule inet fw4 {FORWARD_CHAIN} iifname \"{INTERFACE}\" oifname \"$host_if\" ip saddr {target} ip daddr \"$host_ip\" accept; \
         nft add table inet {FORWARD_TABLE}; \
         nft 'add chain inet {FORWARD_TABLE} prerouting {{ type nat hook prerouting priority -101; policy accept; }}'; \
         nft 'add chain inet {FORWARD_TABLE} postrouting {{ type nat hook postrouting priority 101; policy accept; }}'; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" udp dport {UDP_RX_PORT} dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" udp dport {{ {UDP_TX_PORT}, {UDP_TX_SECONDARY_PORT} }} dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} udp dport {UDP_RX_PORT} snat ip to {client}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} udp dport {{ {UDP_TX_PORT}, {UDP_TX_SECONDARY_PORT} }} snat ip to {client}; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" icmp type echo-request dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} icmp type echo-request snat ip to {client}; \
         trap - EXIT; \
         printf 'forward_address=%s\\n' \"$management_ip\"",
        UDP_RX_PORT = 4_323,
        UDP_TX_PORT = 4_324,
        UDP_TX_SECONDARY_PORT = 4_325,
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "cannot install scoped OpenWrt AP forwarding: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    tagged_ipv4(&String::from_utf8(output.stdout)?, "forward_address")
}

fn snapshot_link(fixture: &OpenWrtConfig) -> Result<OpenWrtClientLinkSnapshot> {
    let script = format!(
        "set -eu; \
         stats=$(iw dev {INTERFACE} station dump) || {{ echo 'link-snapshot: station dump failed' >&2; exit 1; }}; \
         if test -z \"$stats\"; then echo 'link-snapshot: no associated station' >&2; exit 1; fi; \
         set -- /sys/kernel/debug/ieee80211/*/netdev:{INTERFACE}/stations/*/aqm; \
         if test \"$#\" -ne 1 || ! test -r \"$1\"; then echo 'link-snapshot: AQM counters unavailable or ambiguous' >&2; exit 1; fi; aqm=\"$1\"; \
         printf 'rx_bytes=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx bytes:/ {{print $3}}')\"; \
         printf 'rx_packets=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx packets:/ {{print $3}}')\"; \
         printf 'rx_duration=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx duration:/ {{print $3}}')\"; \
         printf 'rx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*rx bitrate:[[:space:]]*//p')\"; \
         printf 'tx_bytes=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx bytes:/ {{print $3}}')\"; \
         printf 'tx_packets=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx packets:/ {{print $3}}')\"; \
         printf 'tx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*tx bitrate:[[:space:]]*//p')\"; \
         printf 'tx_retries=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx retries:/ {{print $3}}')\"; \
         printf 'tx_failed=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx failed:/ {{print $3}}')\"; \
         printf 'tx_duration=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx duration:/ {{print $3}}')\"; \
         printf 'tid0_aqm_drops=%s\\n' \"$(awk '$1 == 0 {{print $6}}' \"$aqm\")\"; \
         printf 'rx_drop_misc=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx drop misc:/ {{print $4}}')\"; \
         printf 'signal_avg=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*signal avg:/ {{print $3}}')\"; \
         iw dev {INTERFACE} survey dump | awk '\
           /frequency:/ {{ used = ($0 ~ /in use/) }} \
           used && /noise:/ {{ print \"survey_noise=\" $2 }} \
           used && /channel active time:/ {{ print \"survey_active=\" $4 }} \
           used && /channel busy time:/ {{ print \"survey_busy=\" $4 }} \
           used && /channel receive time:/ {{ print \"survey_receive=\" $4 }} \
           used && /channel BSS receive time:/ {{ print \"survey_bss_receive=\" $5 }} \
           used && /channel transmit time:/ {{ print \"survey_transmit=\" $4 }}'"
    );
    parse_link_snapshot(ssh(fixture, &script)?)
}

fn parse_link_snapshot(output: std::process::Output) -> Result<OpenWrtClientLinkSnapshot> {
    if !output.status.success() {
        return Err(format!(
            "cannot snapshot controlled OpenWrt AP-client link counters: status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    let output = String::from_utf8(output.stdout)?;
    Ok(OpenWrtClientLinkSnapshot {
        rx_bytes: tagged_u64(&output, "rx_bytes")?,
        rx_packets: tagged_u64(&output, "rx_packets")?,
        rx_duration_micros: tagged_optional_u64(&output, "rx_duration")?,
        rx_bitrate: tagged_optional_string(&output, "rx_bitrate"),
        tx_bytes: tagged_u64(&output, "tx_bytes")?,
        tx_packets: tagged_u64(&output, "tx_packets")?,
        tx_bitrate: tagged_optional_string(&output, "tx_bitrate"),
        tx_retries: tagged_u64(&output, "tx_retries")?,
        tx_failed: tagged_u64(&output, "tx_failed")?,
        tx_duration_micros: tagged_u64(&output, "tx_duration")?,
        tid0_aqm_drops: tagged_u64(&output, "tid0_aqm_drops")?,
        rx_drop_misc: tagged_u64(&output, "rx_drop_misc")?,
        signal_avg_dbm: tagged_optional_i32(&output, "signal_avg")?,
        survey: OpenWrtClientChannelSurvey {
            active_millis: tagged_u64(&output, "survey_active")?,
            busy_millis: tagged_u64(&output, "survey_busy")?,
            receive_millis: tagged_u64(&output, "survey_receive")?,
            bss_receive_millis: tagged_u64(&output, "survey_bss_receive")?,
            transmit_millis: tagged_u64(&output, "survey_transmit")?,
            noise_dbm: tagged_optional_i32(&output, "survey_noise")?,
        },
    })
}

fn tagged_u64(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt output omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
}

fn tagged_optional_u64(output: &str, key: &str) -> Result<Option<u64>> {
    tagged_optional_string(output, key)
        .map(|value| {
            value
                .parse()
                .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
        })
        .transpose()
}

fn tagged_optional_i32(output: &str, key: &str) -> Result<Option<i32>> {
    tagged_optional_string(output, key)
        .map(|value| {
            value
                .parse()
                .map_err(|error| format!("invalid OpenWrt `{key}` value: {error}").into())
        })
        .transpose()
}

fn tagged_optional_string(output: &str, key: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn counter_delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        crate::Error::new(format!(
            "controlled OpenWrt AP-client `{name}` counter reset during the workload: {before} -> {after}"
        ))
        .into()
    })
}

fn optional_counter_delta(
    name: &str,
    before: Option<u64>,
    after: Option<u64>,
) -> Result<Option<u64>> {
    match (before, after) {
        (Some(before), Some(after)) => counter_delta(name, before, after).map(Some),
        (None, None) => Ok(None),
        _ => Err(crate::Error::new(format!(
            "controlled OpenWrt AP-client `{name}` counter availability changed during the workload"
        ))
        .into()),
    }
}

fn cleanup_forwarding_script() -> String {
    format!(
        r#"cleanup_forwarding_resources() {{
            tables=$(nft list tables) || return 1
            if printf '%s\n' "$tables" | grep -Fxq 'table inet {FORWARD_TABLE}'; then
                nft delete table inet {FORWARD_TABLE} || return 1
                tables=$(nft list tables) || return 1
                if printf '%s\n' "$tables" | grep -Fxq 'table inet {FORWARD_TABLE}'; then
                    echo 'OpenWrt client NAT table remained after cleanup' >&2; return 1
                fi
            fi
            if printf '%s\n' "$tables" | grep -Fxq 'table inet fw4'; then
                rules=$(nft -a list table inet fw4) || return 1
                if printf '%s\n' "$rules" | awk '$1 == "chain" && $2 == "{FORWARD_CHAIN}" {{found=1}} END {{exit !found}}'; then
                    forward=$(nft -a list chain inet fw4 forward) || return 1
                    handles=$(printf '%s\n' "$forward" | awk '{{for (i=1; i<NF; i++) if ($i == "jump" && $(i+1) == "{FORWARD_CHAIN}") {{for (j=i+2; j<NF; j++) if ($j == "handle") print $(j+1)}}}}')
                    for handle in $handles; do
                        nft delete rule inet fw4 forward handle "$handle" || return 1
                    done
                    nft flush chain inet fw4 {FORWARD_CHAIN} || return 1
                    nft delete chain inet fw4 {FORWARD_CHAIN} || return 1
                    rules=$(nft -a list table inet fw4) || return 1
                    if printf '%s\n' "$rules" | awk '$1 == "chain" && $2 == "{FORWARD_CHAIN}" {{found=1}} END {{exit !found}}'; then
                        echo 'OpenWrt client forwarding chain remained after cleanup' >&2; return 1
                    fi
                fi
            fi
        }}; cleanup_forwarding_resources"#,
    )
}

fn tagged_ipv4(output: &str, key: &str) -> Result<Ipv4Addr> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt output omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` address: {error}").into())
}

fn derive_psk(ssid: &str, passphrase: &str) -> Result<Zeroizing<String>> {
    let mut child = oer_process::command("wpa_passphrase")
        .arg(ssid)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_owned()?
        .with_timeout(Duration::from_secs(120));
    child
        .stdin
        .take()
        .ok_or("wpa_passphrase has no stdin")?
        .write_all(passphrase.as_bytes())?;
    let mut output = child.wait_with_output()?;
    if !output.status.success() {
        output.stdout.zeroize();
        output.stderr.zeroize();
        return Err("wpa_passphrase failed for the secondary AP client".into());
    }
    let text = Zeroizing::new(String::from_utf8(core::mem::take(&mut output.stdout))?);
    let psk = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("psk="))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("wpa_passphrase did not return a 256-bit PSK")?;
    Ok(Zeroizing::new(psk.to_owned()))
}

pub(crate) fn restore(fixture: &OpenWrtConfig) -> Result<()> {
    let cleanup = cleanup_forwarding_script();
    let script = format!(
        r#"set -u
        failed=0
        {cleanup} || failed=1
        cleanup_client_interface() {{
            if test -f {PID_FILE}; then
                pid=$(cat {PID_FILE}) || return 1
                case "$pid" in ''|*[!0-9]*) echo 'invalid OpenWrt client PID' >&2; return 1;; esac
                test "$pid" -gt 1 || return 1
                if test -d /proc/"$pid"; then
                    arguments=$(cat /proc/"$pid"/cmdline | tr '\000' '\n') || return 1
                    if ! printf '%s\n' "$arguments" | awk '
                        NR == 1 {{count=split($0, path, "/"); executable=(path[count] == "wpa_supplicant")}}
                        previous == "-i" && $0 == "{INTERFACE}" {{interface=1}}
                        previous == "-c" && $0 == "{CONFIG_FILE}" {{config=1}}
                        {{previous=$0}}
                        END {{exit !(executable && interface && config)}}'; then
                        echo 'OpenWrt client PID belongs to another command; leaving it intact' >&2; return 1
                    fi
                    kill "$pid" || return 1
                    for attempt in $(seq 1 5); do
                        test -d /proc/"$pid" || break
                        sleep 1 || return 1
                    done
                    if test -d /proc/"$pid"; then
                        echo 'OpenWrt client process remained after cleanup' >&2; return 1
                    fi
                fi
            fi
            interfaces=$(iw dev) || return 1
            if printf '%s\n' "$interfaces" | awk '$1 == "Interface" && $2 == "{INTERFACE}" {{found=1}} END {{exit !found}}'; then
                iw dev {INTERFACE} del || return 1
                interfaces=$(iw dev) || return 1
                if printf '%s\n' "$interfaces" | awk '$1 == "Interface" && $2 == "{INTERFACE}" {{found=1}} END {{exit !found}}'; then
                    echo 'OpenWrt client interface remained after cleanup' >&2; return 1
                fi
            fi
            rm -f {PID_FILE} {CONFIG_FILE} || return 1
        }}
        cleanup_client_interface || failed=1
        test "$failed" -eq 0"#,
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "cannot restore OpenWrt secondary AP client: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
        .into());
    }
    Ok(())
}

fn probe(
    fixture: &OpenWrtConfig,
    target: Ipv4Addr,
    duration: Duration,
) -> Result<SecondaryClientProbeEvidence> {
    // OpenWrt's compact BusyBox ping accepts integer-second intervals only.
    // One packet per second is sufficient for this liveness stream and keeps
    // it active for the complete primary saturation interval.
    let packets = duration.as_secs().clamp(2, 300) as u32;
    let deadline = duration.as_secs().saturating_add(5).max(5);
    let script =
        format!("ping -4 -q -I {INTERFACE} -c {packets} -i 1 -W 2 -w {deadline} {target}",);
    let output = ssh_with_timeout(
        fixture,
        &script,
        Duration::from_secs(deadline.saturating_add(5)),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let evidence = parse_ping_summary(&stdout).ok_or_else(|| {
        crate::Error::new(format!(
            "cannot parse secondary-client ping evidence: stdout={} stderr={} status={}",
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
            output.status,
        ))
    })?;
    if !output.status.success() || evidence.transmitted != evidence.received {
        return Err(format!(
            "secondary AP client lost traffic: transmitted={} received={} status={}",
            evidence.transmitted, evidence.received, output.status,
        )
        .into());
    }
    Ok(evidence)
}

fn parse_ping_summary(output: &str) -> Option<SecondaryClientProbeEvidence> {
    let summary = output
        .lines()
        .find(|line| line.contains("packets transmitted") && line.contains("packets received"))?;
    let mut fields = summary.split(',');
    let transmitted = fields.next()?.split_whitespace().next()?.parse().ok()?;
    let received = fields.next()?.split_whitespace().next()?.parse().ok()?;
    Some(SecondaryClientProbeEvidence {
        transmitted,
        received,
    })
}

fn ssh(fixture: &OpenWrtConfig, script: &str) -> Result<std::process::Output> {
    ssh_with_timeout(fixture, script, Duration::from_secs(120))
}

fn ssh_with_timeout(
    fixture: &OpenWrtConfig,
    script: &str,
    timeout: Duration,
) -> Result<std::process::Output> {
    oer_process::output(
        &mut oer_stand_ssh::command(&fixture.ssh_target, script),
        Some(timeout),
    )
    .and_then(crate::Error::ssh_output)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests;
