//! Ordinary ICMP echo: the host's own `ping` to the image, answered by the
//! image's IP stack, one request each [`PERIOD`] and a report of the same
//! shape as the RTT probes'.

use std::{net::Ipv4Addr, process::Command, time::Duration};

use crate::traffic::Echoes;

/// The time between two requests, the shortest an unprivileged `ping` need
/// not justify.
pub const PERIOD: Duration = Duration::from_millis(100);
/// How long a request waits for its reply.
const REPLY_TIMEOUT_S: &str = "1";

/// Send `count` requests of `ping`'s ordinary 56 bytes to `to`.
pub fn echo(to: Ipv4Addr, count: u32) -> Result<Echoes, String> {
    let output = Command::new("ping")
        .env("LC_ALL", "C")
        .args(["-n", "-W", REPLY_TIMEOUT_S, "-i"])
        .arg(format!("{:.3}", PERIOD.as_secs_f64()))
        .arg("-c")
        .arg(count.to_string())
        .arg(to.to_string())
        .output()
        .map_err(|error| format!("ping: {error}"))?;
    Ok(Echoes {
        sent: u64::from(count),
        rtt_us: replies(&String::from_utf8_lossy(&output.stdout)),
    })
}

/// The round trips of `ping`'s reply lines, in arrival order. A duplicate
/// reply answers a request already counted.
fn replies(text: &str) -> Vec<u64> {
    text.lines()
        .filter(|line| !line.contains("(DUP!)"))
        .filter_map(round_trip_us)
        .collect()
}

/// The `time=2.46 ms` of a reply line in microseconds.
fn round_trip_us(line: &str) -> Option<u64> {
    let milliseconds = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix("time="))?
        .parse::<f64>()
        .ok()?;
    Some((milliseconds * 1000.0).round() as u64)
}

/// The median of `echoes`' replies; `None` without one.
pub fn median_us(echoes: &Echoes) -> Option<u64> {
    let mut sorted = echoes.rtt_us.clone();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "PING 10.231.0.20 (10.231.0.20) 56(84) bytes of data.
64 bytes from 10.231.0.20: icmp_seq=1 ttl=63 time=2.46 ms
64 bytes from 10.231.0.20: icmp_seq=1 ttl=63 time=9.10 ms (DUP!)
64 bytes from 10.231.0.20: icmp_seq=2 ttl=63 time=0.345 ms
From 10.231.0.1 icmp_seq=3 Destination Host Unreachable
64 bytes from 10.231.0.20: icmp_seq=4 ttl=63 time=12 ms

--- 10.231.0.20 ping statistics ---
4 packets transmitted, 3 received, 25% packet loss, time 303ms
rtt min/avg/max/mdev = 0.345/4.935/12.000/5.113 ms
";

    #[test]
    fn replies_are_the_reply_lines_without_duplicates() {
        assert_eq!(replies(OUTPUT), [2460, 345, 12_000]);
    }

    #[test]
    fn a_line_without_a_round_trip_counts_nothing() {
        assert_eq!(
            replies("From 10.231.0.1 icmp_seq=3 time=x ms\n"),
            [0_u64; 0]
        );
    }

    #[test]
    fn the_median_needs_a_reply() {
        let of = |rtt_us: &[u64]| {
            median_us(&Echoes {
                sent: 3,
                rtt_us: rtt_us.to_vec(),
            })
        };
        assert_eq!(of(&[]), None);
        assert_eq!(of(&[900, 5000, 2000]), Some(2000));
    }
}
