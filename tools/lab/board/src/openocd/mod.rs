//! The one OpenOCD of the stand: located in the ESP-IDF tools of the shared
//! cache, it resets a chip through its builtin USB-JTAG, reads a halted
//! hart's registers and programs flash.
//!
//! A USB Serial/JTAG reset restarts only the HP system, and some chips then
//! misbehave (see `docs/hardware-errata.md`).
//! OpenOCD reaches the same USB device's JTAG interface, writes flash through
//! its flasher stub and resets the CPU through the debug module, which none of
//! those faults affect. A board is named by its MAC (`adapter serial`).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Longest an OpenOCD flash may take; programming several megabytes over
/// USB JTAG takes about a minute.
const PROGRAM_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest a reset may take.
pub const RESET_TIMEOUT: Duration = Duration::from_secs(60);

/// The OpenOCD build that reaches a chip's builtin USB-JTAG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Openocd {
    pub program: PathBuf,
    pub scripts: PathBuf,
}

impl Openocd {
    /// The newest OpenOCD of the ESP-IDF tools in the shared cache, which any
    /// ESP-IDF catalog build installs.
    pub fn locate() -> crate::Result<Self> {
        Self::locate_in(&oer_os_durable::xdg::esp_idf_cache()?.join("idf-tools"))
    }

    fn locate_in(idf_tools: &Path) -> crate::Result<Self> {
        let tools = idf_tools.join("tools/openocd-esp32");
        let mut versions = std::fs::read_dir(&tools)
            .map_err(|_| {
                format!(
                    "no OpenOCD in {}; build any ESP-IDF catalog image once (`cargo fw idf \
                     build IMAGE`) to install the tools",
                    tools.display()
                )
            })?
            .flatten()
            .map(|entry| entry.path().join("openocd-esp32"))
            .filter(|root| root.join("bin/openocd").is_file())
            .collect::<Vec<_>>();
        versions.sort();
        let root = versions
            .pop()
            .ok_or("the ESP-IDF tools hold no OpenOCD build")?;
        Ok(Self {
            program: root.join("bin/openocd"),
            scripts: root.join("share/openocd/scripts"),
        })
    }

    fn board(&self, chip: &str, mac: &str) -> Vec<String> {
        vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-f"),
            format!("board/{chip}-builtin.cfg"),
            String::from("-c"),
            format!("adapter serial {mac}"),
        ]
    }

    /// The arguments of a `reset run` of the board of `chip` with `mac`.
    pub fn reset_arguments(&self, chip: &str, mac: &str) -> Vec<String> {
        let mut arguments = self.board(chip, mac);
        for command in ["init", "reset run", "shutdown"] {
            arguments.extend([String::from("-c"), String::from(command)]);
        }
        arguments
    }

    /// Reset the chip through its builtin USB-JTAG and let it run. On a
    /// staged chip this is a software system reset (`rst:0x3`) that also clears
    /// low-power and PMU state which an RTS reset through the USB
    /// Serial/JTAG port, and a reflash, leave in place (a powered-down MPLL
    /// kept the second-stage bootloader in a watchdog loop until it).
    pub fn reset(
        &self,
        chip: &str,
        mac: &str,
        lifetime: &oer_os_process::IoLifetime,
    ) -> crate::Result<()> {
        self.run(&self.reset_arguments(chip, mac), RESET_TIMEOUT, lifetime)
            .map(drop)
    }

    /// The arguments that halt the chip, print `registers` and let it run
    /// on, with no reset and no debugger server listening.
    pub fn register_arguments(&self, chip: &str, mac: &str, registers: &[&str]) -> Vec<String> {
        let mut commands = String::from("init; halt");
        for register in registers {
            commands.push_str(&format!("; echo \"{register} [reg {register}]\""));
        }
        commands.push_str("; resume; shutdown");
        let mut arguments = vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-c"),
            String::from("gdb_port disabled; telnet_port disabled; tcl_port disabled"),
        ];
        arguments.extend(self.board(chip, mac).into_iter().skip(2));
        arguments.extend([String::from("-c"), commands]);
        arguments
    }

    /// Halt the chip, read `registers` of its current hart and let it run
    /// on: what a target that stopped answering was doing, without the reset
    /// that would erase it.
    pub fn registers(
        &self,
        chip: &str,
        mac: &str,
        registers: &[&str],
        timeout: Duration,
        lifetime: &oer_os_process::IoLifetime,
    ) -> crate::Result<Vec<(String, u32)>> {
        let mut command = oer_os_process::command(&self.program);
        command.args(self.register_arguments(chip, mac, registers));
        lifetime.pin(&mut command)?;
        let output = oer_os_process::output(&mut command, Some(timeout))?;
        let log = String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout);
        let values = parse_registers(&log, registers);
        if values.is_empty() {
            return Err(format!(
                "OpenOCD read no register ({}):\n{}",
                output.status,
                tail(&log, 12)
            )
            .into());
        }
        Ok(values)
    }

    /// The arguments that halt the chip, print the 32-bit word at `address`
    /// and let it run on, with no reset and no debugger server listening.
    pub fn word_arguments(&self, chip: &str, mac: &str, address: u32) -> Vec<String> {
        let mut arguments = vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-c"),
            String::from("gdb_port disabled; telnet_port disabled; tcl_port disabled"),
        ];
        arguments.extend(self.board(chip, mac).into_iter().skip(2));
        arguments.extend([
            String::from("-c"),
            format!("init; halt; echo \"{WORD} [mdw {address:#010x}]\"; resume; shutdown"),
        ]);
        arguments
    }

    /// Halt the chip, read the 32-bit word at `address` and let it run on.
    pub fn read_word(
        &self,
        chip: &str,
        mac: &str,
        address: u32,
        timeout: Duration,
        lifetime: &oer_os_process::IoLifetime,
    ) -> crate::Result<u32> {
        let mut command = oer_os_process::command(&self.program);
        command.args(self.word_arguments(chip, mac, address));
        lifetime.pin(&mut command)?;
        let output = oer_os_process::output(&mut command, Some(timeout))?;
        let log = String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout);
        parse_word(&log, address).ok_or_else(|| {
            format!(
                "OpenOCD read no word at {address:#010x} ({}):\n{}",
                output.status,
                tail(&log, 12)
            )
            .into()
        })
    }

    /// The arguments that write `files` (flash offset, file), verify each
    /// and reset the CPU after the last.
    pub fn program_arguments(
        &self,
        chip: &str,
        mac: &str,
        files: &[(u32, PathBuf)],
    ) -> Vec<String> {
        let mut arguments = self.board(chip, mac);
        for (index, (offset, file)) in files.iter().enumerate() {
            let last = index + 1 == files.len();
            arguments.push(String::from("-c"));
            arguments.push(format!(
                "program_esp {} {offset:#x} verify{}",
                file.display(),
                if last { " reset exit" } else { "" }
            ));
        }
        arguments
    }

    /// Write `files` to the board and reset it into the application.
    pub fn program(
        &self,
        chip: &str,
        mac: &str,
        files: &[(u32, PathBuf)],
        lifetime: &oer_os_process::IoLifetime,
    ) -> crate::Result<()> {
        if files.is_empty() {
            return Err("nothing to flash".into());
        }
        self.run(
            &self.program_arguments(chip, mac, files),
            PROGRAM_TIMEOUT,
            lifetime,
        )
        .map(drop)
    }

    /// Run OpenOCD with `arguments`, terminating it after `timeout`: a debug
    /// session on a wedged target can block forever, even against `SIGTERM`.
    /// Its log is shown only when it fails.
    fn run(
        &self,
        arguments: &[String],
        timeout: Duration,
        lifetime: &oer_os_process::IoLifetime,
    ) -> crate::Result<std::process::Output> {
        let mut command = oer_os_process::command(&self.program);
        command.args(arguments);
        lifetime.pin(&mut command)?;
        let output = oer_os_process::output(&mut command, Some(timeout)).map_err(|error| {
            if error.is::<oer_os_process::owned::DeadlineExceeded>() {
                format!(
                    "OpenOCD did not finish within {}s and was stopped",
                    timeout.as_secs()
                )
                .into()
            } else {
                error
            }
        })?;
        if !output.status.success() {
            return Err(format!(
                "OpenOCD failed with {}:\n{}",
                output.status,
                tail(&String::from_utf8_lossy(&output.stderr), 20)
            )
            .into());
        }
        Ok(output)
    }
}

fn tail(log: &str, lines: usize) -> String {
    let mut tail = log.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    tail.join("\n")
}

/// The label of the line [`Openocd::word_arguments`] echoes.
const WORD: &str = "word";

/// The word of the `word 0x20701030: 00000003` line OpenOCD echoed for
/// `address`.
fn parse_word(log: &str, address: u32) -> Option<u32> {
    let line = log
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(&format!("{WORD} ")))?;
    let (at, value) = line.split_once(':')?;
    if u32::from_str_radix(at.trim().strip_prefix("0x")?, 16).ok()? != address {
        return None;
    }
    u32::from_str_radix(value.split_whitespace().next()?, 16).ok()
}

/// The `NAME name (/32): 0x…` lines OpenOCD echoed for `registers`.
fn parse_registers(log: &str, registers: &[&str]) -> Vec<(String, u32)> {
    registers
        .iter()
        .filter_map(|register| {
            let line = log
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{register} ")))?;
            let value = line.rsplit(':').next()?.trim();
            let value = u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()?;
            Some((String::from(*register), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openocd() -> Openocd {
        Openocd {
            program: "openocd".into(),
            scripts: "/scripts".into(),
        }
    }

    #[test]
    fn every_file_is_verified_and_only_the_last_resets() {
        let arguments = openocd().program_arguments(
            "chip-b",
            "02:00:00:00:25:64",
            &[(0x2000, "boot.bin".into()), (0x10000, "app.bin".into())],
        );
        assert_eq!(
            arguments,
            [
                "-s",
                "/scripts",
                "-f",
                "board/chip-b-builtin.cfg",
                "-c",
                "adapter serial 02:00:00:00:25:64",
                "-c",
                "program_esp boot.bin 0x2000 verify",
                "-c",
                "program_esp app.bin 0x10000 verify reset exit",
            ]
        );
    }

    #[test]
    fn a_reset_names_the_board_by_its_mac_and_runs_it() {
        assert_eq!(
            openocd().reset_arguments("chip-a", "02:00:00:00:F6:D0"),
            [
                "-s",
                "/scripts",
                "-f",
                "board/chip-a-builtin.cfg",
                "-c",
                "adapter serial 02:00:00:00:F6:D0",
                "-c",
                "init",
                "-c",
                "reset run",
                "-c",
                "shutdown",
            ]
        );
    }

    #[test]
    fn registers_are_read_without_a_reset_and_a_missing_one_is_left_out() {
        let log = "Info : [chip-b] Target halted, PC=0x42010736\n\
                   pc pc (/32): 0x42010736\n\
                   mcause mcause (/32): 0x30000007\n\
                   Info : shutdown command invoked\n";
        assert_eq!(
            parse_registers(log, &["pc", "mcause", "mtval"]),
            [
                (String::from("pc"), 0x4201_0736),
                (String::from("mcause"), 0x3000_0007)
            ]
        );
        let arguments = openocd().register_arguments("chip-b", "02:00:00:00:25:64", &["pc"]);
        assert_eq!(
            &arguments[..4],
            [
                "-s",
                "/scripts",
                "-c",
                "gdb_port disabled; telnet_port disabled; tcl_port disabled"
            ]
        );
        assert!(arguments.contains(&String::from("adapter serial 02:00:00:00:25:64")));
        let commands = arguments.last().unwrap();
        // It reads without resetting and leaves the chip running.
        assert!(commands.contains("halt") && commands.ends_with("resume; shutdown"));
        assert!(!commands.contains("reset"));
    }

    #[test]
    fn a_word_is_read_without_a_reset_at_its_own_address() {
        let log = "Info : [esp32s31.cpu0] Target halted, PC=0x2f81f5a2\n\
                   word 0x20701030: 00000003 \n\
                   Info : shutdown command invoked\n";
        assert_eq!(parse_word(log, 0x2070_1030), Some(3));
        assert_eq!(parse_word(log, 0x2070_1034), None);
        assert_eq!(
            parse_word("Error: Target not examined yet\n", 0x2070_1030),
            None
        );
        let arguments = openocd().word_arguments("chip-b", "02:00:00:00:25:64", 0x600b_0410);
        assert!(arguments.contains(&String::from("adapter serial 02:00:00:00:25:64")));
        let commands = arguments.last().unwrap();
        assert!(commands.contains("mdw 0x600b0410"), "{commands}");
        assert!(commands.ends_with("resume; shutdown") && !commands.contains("reset"));
    }

    #[test]
    fn the_newest_openocd_of_the_idf_tools_is_located() {
        let tools = tempfile::tempdir().unwrap();
        assert!(Openocd::locate_in(tools.path()).is_err());
        for version in ["v0.12.0-esp32-20240318", "v0.12.0-esp32-20250707"] {
            let root = tools
                .path()
                .join("tools/openocd-esp32")
                .join(version)
                .join("openocd-esp32");
            std::fs::create_dir_all(root.join("bin")).unwrap();
            std::fs::write(root.join("bin/openocd"), "").unwrap();
        }
        let located = Openocd::locate_in(tools.path()).unwrap();
        assert!(located.program.to_string_lossy().contains("20250707"));
        assert!(located.scripts.ends_with("share/openocd/scripts"));
    }
}
