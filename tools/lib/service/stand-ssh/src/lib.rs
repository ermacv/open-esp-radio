//! The one way the host reaches the stand's OpenWrt hosts: non-interactive
//! SSH (`BatchMode`, a five-second connect timeout) to the target the stand
//! file names, running one shell script. Callers supervise the command as
//! they need (`oer_process::output` with a deadline, `supervised_output`, a
//! capture process).

#![forbid(unsafe_code)]

use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// `ssh` running `script` on `target`.
pub fn command(target: &str, script: &str) -> Command {
    let mut command = oer_process::command("ssh");
    command
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(target)
        .arg(script);
    command
}

/// The script that prints the host's boot identity.
pub const BOOT_ID: &str = "cat /proc/sys/kernel/random/boot_id";

/// The resource key a lease claims an OpenWrt host by, from the boot
/// identity it printed: one owner covers the whole boot of the host,
/// whichever SSH alias or radio interface a caller uses.
pub fn host_key(boot_id: &str) -> Result<String> {
    let identity = boot_id.trim();
    if identity.len() != 36
        || !identity.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err("OpenWrt host did not return a valid boot identity".into());
    }
    Ok(format!(
        "openwrt-host-boot:{}",
        identity.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_non_interactive_and_names_its_target() {
        let command = command("open-radio-ap", "iw dev");
        assert_eq!(command.get_program(), "ssh");
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "open-radio-ap",
                "iw dev"
            ]
        );
    }

    #[test]
    fn a_host_is_claimed_by_its_boot() {
        assert_eq!(
            host_key("9A1B2C3D-0000-4000-8000-00000000000A\n").unwrap(),
            "openwrt-host-boot:9a1b2c3d-0000-4000-8000-00000000000a"
        );
        assert!(host_key("not a boot id").is_err());
    }
}
