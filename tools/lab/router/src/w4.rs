//! Router actions for WPA2-Personal station runs. Credentials are write-only
//! through [`crate::router::Profile::w4`]; no raw remote answer is evidence.

use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result, token},
};
use zeroize::Zeroizing;

/// A short GTK lifetime for a run's 100-echo traffic window. The lab profile
/// is committed without restoration, just like the other station settings.
pub const GROUP_REKEY_SECONDS: u32 = 30;

/// Read-only admission check for the W4 observation tools. Fixture software
/// installation is separate from a qualification run.
pub fn verify_observation_tools(remote: &mut dyn Remote) -> Result<()> {
    let _hidden=Zeroizing::new(remote.run(
        "command -v arping >/dev/null 2>&1 && command -v tcpdump >/dev/null 2>&1")
        .map_err(|_| "W4 requires arping and tcpdump on the managed router; fixture setup is outside this run")?);
    Ok(())
}

/// Check the running BSS, filtering every value on the router before it
/// reaches stdout. Configuration keys and passphrases are never read back.
pub fn verify_runtime(remote: &mut dyn Remote, radio: &str, interface: &str) -> Result<()> {
    if !token(radio) || !token(interface) {
        return Err("invalid W4 runtime profile selection".to_owned());
    }
    let script = format!(
        "set +x\nset -e\nphy=$(ubus call iwinfo phyname {} | jsonfilter -e '@.phyname')\n\
        case \"$phy\" in phy[0-9]*) ;; *) exit 1;; esac\n\
        awk -F= -v iface={} '\n\
        $1==\"interface\" || $1==\"bss\" {{selected=($2==iface); if(selected) found++}}\n\
        $1==\"ieee80211n\" {{n=$2}}\n$1==\"ieee80211ax\" {{ax=$2}}\n\
        $1==\"ht_capab\" {{wide=($2 ~ /HT40/)}}\n\
        selected && $1==\"wpa\" {{wpa=$2}}\n\
        selected && $1==\"wpa_key_mgmt\" {{psk=($2==\"WPA-PSK\")}}\n\
        selected && $1==\"wpa_pairwise\" {{ccmp=($2==\"CCMP\")}}\n\
        selected && $1==\"ieee80211w\" {{pmf=$2}}\n\
        END {{printf \"%d %d %d %d %d %d %d %d\\n\",found,n,ax,wide,wpa,psk,ccmp,pmf}}' \"/var/run/hostapd-$phy.conf\"",
        quote(&format!("{{\"section\":\"{radio}\"}}"))?,
        quote(interface)?
    );
    let output = remote
        .run(&script)
        .map_err(|_| "W4 runtime profile verification failed")?;
    if output.trim() != "1 1 0 0 2 1 1 0" {
        return Err("running BSS is not WPA2-PSK/CCMP HT20 with HE and PMF disabled".to_owned());
    }
    Ok(())
}

/// Broadcast an ARP request to the station after a completed GTK change.
/// All remote diagnostics remain hidden; reception is proved by target and
/// air observations, never the arping process's success alone.
pub fn broadcast_arp(
    remote: &mut dyn Remote,
    interface: &str,
    address: std::net::Ipv4Addr,
) -> Result<()> {
    if !token(interface) || !crate::w3::usable_address(address) {
        return Err("invalid W4 ARP selection".to_owned());
    }
    let script = format!(
        "set +x\narping -b -I {} -c 1 -w 3 {address} >/dev/null 2>&1",
        quote(interface)?
    );
    let _hidden = Zeroizing::new(remote.run(&script).map_err(|_| "W4 broadcast ARP failed")?);
    Ok(())
}

/// Ask the selected AP to deauthenticate one station for the reconnect gate.
/// The caller holds router and air ownership for the complete run. The full
/// MAC is used only in the stdin script; remote stdout/stderr stay remote.
pub fn deauthenticate(remote: &mut dyn Remote, interface: &str, station: Mac) -> Result<()> {
    if !token(interface) {
        return Err("invalid router AP interface".to_owned());
    }
    let address = station.octets().map(|byte| format!("{byte:02x}")).join(":");
    let script = format!(
        "set +x\n{{\nreply=$(hostapd_cli -i {} deauthenticate {}) && test \"$reply\" = OK\n\
         }} >/dev/null 2>&1",
        quote(interface)?,
        quote(&address)?,
    );
    match remote.run(&script) {
        Ok(output) => {
            let _hidden = Zeroizing::new(output);
            Ok(())
        }
        Err(error) => {
            let _hidden = Zeroizing::new(error);
            Err("router station deauthentication failed".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted {
        answer: Result<String>,
        scripts: Vec<String>,
    }

    impl Remote for Scripted {
        fn run(&mut self, script: &str) -> Result<String> {
            self.scripts.push(script.to_owned());
            self.answer.clone()
        }
    }

    #[test]
    fn runtime_profile_is_numeric_only_and_never_returns_keys_or_diagnostics() {
        let mut remote = Scripted {
            answer: Ok("1 1 0 0 2 1 1 0\n".to_owned()),
            scripts: Vec::new(),
        };
        verify_runtime(&mut remote, "radio1", "phy1-ap0").expect("WPA2 runtime");
        assert!(remote.scripts[0].contains("wpa_pairwise"));
        assert!(!remote.scripts[0].contains("wpa_passphrase"));
        remote.answer = Ok("1 1 0 0 2 1 0 0".to_owned());
        assert!(verify_runtime(&mut remote, "radio1", "phy1-ap0").is_err());
        remote.answer = Err("secret-passphrase PMK PTK GTK".to_owned());
        assert!(
            !verify_runtime(&mut remote, "radio1", "phy1-ap0")
                .expect_err("remote failure")
                .contains("secret-passphrase")
        );
        assert!(
            !broadcast_arp(
                &mut remote,
                "br-oer-w3",
                "10.231.0.100".parse().expect("IP")
            )
            .expect_err("remote failure")
            .contains("secret-passphrase")
        );
    }

    #[test]
    fn missing_fixture_tools_stop_admission_and_errors_reveal_no_remote_output() {
        let mut remote = Scripted {
            answer: Err("private-router private-passphrase".to_owned()),
            scripts: Vec::new(),
        };
        let error = verify_observation_tools(&mut remote).expect_err("fixture tools required");
        assert!(error.contains("arping"));
        assert!(!error.contains("private-"));
        assert_eq!(
            remote.scripts,
            ["command -v arping >/dev/null 2>&1 && command -v tcpdump >/dev/null 2>&1"]
        );
        remote.answer = Ok(String::new());
        verify_observation_tools(&mut remote).expect("read-only admission");
    }

    #[test]
    fn deauthentication_selects_the_ap_and_the_exact_station_and_requires_ok() {
        let mut remote = Scripted {
            answer: Ok(String::new()),
            scripts: Vec::new(),
        };
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        deauthenticate(&mut remote, "phy7-ap3", station).unwrap();
        assert_eq!(remote.scripts.len(), 1);
        let script = &remote.scripts[0];
        assert!(script.contains("hostapd_cli -i phy7-ap3 deauthenticate 02:ab:cd:ef:f6:d0"));
        assert!(script.contains("&& test \"$reply\" = OK"));
        assert!(script.starts_with("set +x\n"));
        assert!(script.ends_with("} >/dev/null 2>&1"));
        assert!(!script.contains("disassociate"));
    }

    #[test]
    fn remote_errors_and_bad_interfaces_never_repeat_identities_or_credentials() {
        let mut remote = Scripted {
            answer: Err("secret-passphrase phy7-ap3 02:ab:cd:ef:f6:d0".to_owned()),
            scripts: Vec::new(),
        };
        let station = Mac::new([0x02, 0xab, 0xcd, 0xef, 0xf6, 0xd0]).unwrap();
        assert_eq!(
            deauthenticate(&mut remote, "phy7-ap3", station).unwrap_err(),
            "router station deauthentication failed"
        );
        for interface in ["phy7-ap3; secret-passphrase", "-i", ""] {
            assert_eq!(
                deauthenticate(&mut remote, interface, station).unwrap_err(),
                "invalid router AP interface"
            );
        }
        assert_eq!(remote.scripts.len(), 1);
    }
}
