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
