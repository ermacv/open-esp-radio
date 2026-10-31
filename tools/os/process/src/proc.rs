//! Linux process identities: `/proc` start time distinguishes recycled
//! PIDs.

/// The fields of `/proc/<pid>/stat` after the command name (field 2), which
/// is enclosed in parentheses and may itself contain spaces or parentheses.
/// The first returned field is field 3 (state).
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    fields_after_command(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn fields_after_command(stat: &str) -> Option<Vec<String>> {
    let (_, fields) = stat.rsplit_once(')')?;
    Some(fields.split_whitespace().map(str::to_owned).collect())
}

/// Field `number` (1-based, as `proc(5)` numbers them, from 3) of a stat line.
fn field(fields: &[String], number: usize) -> Option<u64> {
    fields.get(number.checked_sub(3)?)?.parse().ok()
}

/// When the live process `pid` started, in clock ticks since boot (field 22).
pub fn start_ticks(pid: u32) -> Option<u64> {
    field(&stat_fields(pid)?, 22)
}

/// When the live process `pid` started, in Unix milliseconds, to the
/// resolution of the kernel's boot time (one second).
pub fn started_unix_millis(pid: u32) -> Option<u64> {
    let ticks = start_ticks(pid)?;
    let boot_seconds: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let ticks_per_second = rustix::param::clock_ticks_per_second();
    Some(boot_seconds * 1000 + ticks * 1000 / ticks_per_second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_skip_command_names_with_spaces_and_parentheses() {
        let stat = "42 (a (b) c) S 7 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 19";
        let fields = fields_after_command(stat).unwrap();
        assert_eq!(field(&fields, 4), Some(7));
        assert_eq!(field(&fields, 22), Some(777));
        assert_eq!(
            field(&fields_after_command("42 (x) S 1").unwrap(), 22),
            None
        );
    }
}
