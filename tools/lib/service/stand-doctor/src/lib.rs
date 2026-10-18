//! Whether the host is set up for the stand: the stand file and its chip
//! profiles, the repository's udev rule that lets the stand's user switch
//! hub ports, `uhubctl` reaching every stand hub without sudo, and
//! NetworkManager leaving `wlan0` to the fixtures.
#![forbid(unsafe_code)]

use std::{path::Path, time::Duration};

use oer_stand_file::StandFile;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Where the repository's host files are installed.
const UDEV_RULE: &str = "/etc/udev/rules.d/52-oer-uhubctl.rules";
const NETWORKMANAGER_CONF: &str = "/etc/NetworkManager/conf.d/oer-unmanaged.conf";

/// One check of the stand doctor.
#[derive(Debug, Eq, PartialEq)]
pub struct Check {
    pub name: &'static str,
    pub failure: Option<String>,
}

fn check(name: &'static str, result: Result<()>) -> Check {
    Check {
        name,
        failure: result.err().map(|error| error.to_string()),
    }
}

/// Every check of the host of the repository at `root` with the stand file
/// at `path`.
pub fn doctor(root: &Path, path: &Path) -> Vec<Check> {
    let stand = StandFile::load(path);
    let stand_check = match &stand {
        Ok(stand) => oer_chip_profile::Profile::all(root)
            .map_err(|error| error.to_string().into())
            .and_then(|chips| stand.validate_chips(&chips).map_err(Into::into)),
        Err(error) => Err(error.to_string().into()),
    };
    let mut checks = vec![check("stand file", stand_check)];
    checks.push(check(
        "udev rule",
        same_file(
            &root.join("stand/udev/52-oer-uhubctl.rules"),
            Path::new(UDEV_RULE),
        )
        .map_err(|error| format!("{error}; install it with sudo and reload udev").into()),
    ));
    checks.push(check(
        "uhubctl without sudo",
        match &stand {
            Ok(stand) => uhubctl_reaches(stand),
            Err(_) => Err("needs the stand file's hubs".into()),
        },
    ));
    checks.push(check(
        "NetworkManager configuration",
        same_file(
            &root.join("stand/networkmanager/oer-unmanaged.conf"),
            Path::new(NETWORKMANAGER_CONF),
        )
        .map_err(|error| format!("{error}; install it with sudo and reload NetworkManager").into()),
    ));
    checks.push(check("wlan0 unmanaged", wlan0_unmanaged()));
    checks
}

/// Whether `installed` holds the repository's `source`.
pub fn same_file(source: &Path, installed: &Path) -> Result<()> {
    let wanted = std::fs::read(source)?;
    match std::fs::read(installed) {
        Ok(found) if found == wanted => Ok(()),
        Ok(_) => Err(format!("{} differs from {}", installed.display(), source.display()).into()),
        Err(_) => Err(format!("{} is not installed", installed.display()).into()),
    }
}

/// `uhubctl` reads every stand hub without sudo.
fn uhubctl_reaches(stand: &StandFile) -> Result<()> {
    let hubs = oer_stand_power::report()?;
    let unreached = stand
        .hub
        .iter()
        .filter(|hub| !hubs.iter().any(|status| status.location == hub.usb2))
        .map(|hub| format!("{} ({})", hub.id, hub.usb2))
        .collect::<Vec<_>>();
    if unreached.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "uhubctl does not report {}: not attached, or not readable without sudo",
            unreached.join(", ")
        )
        .into())
    }
}

fn wlan0_unmanaged() -> Result<()> {
    let output = oer_process::output(
        oer_process::command("nmcli").args(["-t", "-f", "DEVICE,STATE", "device"]),
        Some(Duration::from_secs(10)),
    )?;
    wlan0_state(&String::from_utf8_lossy(&output.stdout))
}

/// `wlan0`'s state in `nmcli -t -f DEVICE,STATE device`: it must be
/// unmanaged, or absent from NetworkManager.
pub fn wlan0_state(report: &str) -> Result<()> {
    match report
        .lines()
        .find_map(|line| line.strip_prefix("wlan0:"))
        .map(str::trim)
    {
        None | Some("unmanaged") => Ok(()),
        Some(state) => Err(format!("NetworkManager manages wlan0 ({state})").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wlan0_must_be_left_to_the_fixtures() {
        assert!(wlan0_state("wlan0:unmanaged\nlo:unmanaged\n").is_ok());
        assert!(wlan0_state("eth0:connected\n").is_ok(), "no wlan0 at all");
        let error = wlan0_state("wlan0:disconnected\n").unwrap_err().to_string();
        assert!(error.contains("manages wlan0 (disconnected)"), "{error}");
    }

    #[test]
    fn an_installed_host_file_matches_the_repository() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let installed = directory.path().join("installed");
        std::fs::write(&source, "rule\n").unwrap();
        assert!(
            same_file(&source, &installed)
                .unwrap_err()
                .to_string()
                .contains("not installed")
        );
        std::fs::write(&installed, "old rule\n").unwrap();
        assert!(
            same_file(&source, &installed)
                .unwrap_err()
                .to_string()
                .contains("differs")
        );
        std::fs::write(&installed, "rule\n").unwrap();
        same_file(&source, &installed).unwrap();
    }
}
