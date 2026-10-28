//! Conservation of the complete AP-initiated echo workload (warmup/flood/recovery).
use serde::Serialize;
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Row {
    pub stage: &'static str,
    pub frames: u32,
}
fn number(console: &str, topic: &str, key: &str) -> Result<u32, String> {
    let mut lines = console.lines().filter_map(|line| line.strip_prefix(topic));
    let line = lines.next().ok_or_else(|| format!("missing {topic}"))?;
    if lines.next().is_some() {
        return Err(format!("repeated {topic}"));
    }
    let mut fields = line
        .split_whitespace()
        .filter_map(|word| word.split_once('='))
        .filter(|(name, _)| *name == key);
    let (_, value) = fields.next().ok_or_else(|| format!("missing {key}"))?;
    if fields.next().is_some() {
        return Err(format!("repeated {key}"));
    }
    value.parse().map_err(|_| format!("invalid {key}"))
}
pub fn account(console: &str, requests: usize, replies: usize) -> Result<Vec<Row>, String> {
    let ap = u32::try_from(requests).map_err(|_| "too many AP requests")?;
    let ap_replies = u32::try_from(replies).map_err(|_| "too many AP replies")?;
    stages([
        ap,
        number(console, "w3.echo ", "requests")?,
        number(console, "w3.stack ", "echo_requests")?,
        number(console, "w3.stack ", "echo_replies")?,
        number(console, "w3.stack ", "echo_dropped")?,
        number(console, "w3.echo ", "replies")?,
        ap_replies,
    ])
}
fn stages(
    [ap, bridge, stack, formed, dropped, admitted, replies]: [u32; 7],
) -> Result<Vec<Row>, String> {
    let diff = |from: u32, to: u32| {
        from.checked_sub(to)
            .ok_or_else(|| "echo counters do not conserve AP traffic".to_owned())
    };
    Ok(vec![
        Row {
            stage: "AP to bridge (includes service queue-full)",
            frames: diff(ap, bridge)?,
        },
        Row {
            stage: "bridge to validated stack request",
            frames: diff(bridge, stack)?,
        },
        Row {
            stage: "stack request to formed reply",
            frames: diff(stack, formed)?,
        },
        Row {
            stage: "Xarxa EchoReply retention overflow",
            frames: dropped,
        },
        Row {
            stage: "formed reply to service TX, excluding retention overflow",
            frames: diff(diff(formed, dropped)?, admitted)?,
        },
        Row {
            stage: "service TX to AP reply",
            frames: diff(admitted, replies)?,
        },
    ])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_missing_echo_is_counted_once_and_inconsistent_scopes_are_refused() {
        let rows = stages([10000, 7000, 7000, 7000, 3900, 3100, 3043]).unwrap();
        assert_eq!(rows.iter().map(|r| r.frames).sum::<u32>(), 10000 - 3043);
        assert_eq!(rows[0].frames, 3000);
        assert_eq!(rows[3].frames, 3900);
        assert_eq!(rows[5].frames, 57);
        assert!(stages([1, 2, 2, 2, 0, 2, 2]).is_err());
        assert!(stages([2, 2, 2, 2, 3, 0, 0]).is_err());
        assert!(account("", 10000, 3043).is_err());
    }

    #[test]
    fn retried_refusals_are_not_losses_but_overflow_is_counted_once() {
        let console = "w3.echo requests=10 replies=9\n\
                       w3.stack echo_requests=10 echo_replies=10 echo_refused=23 echo_dropped=1";
        let rows = account(console, 12, 9).unwrap();
        assert_eq!(rows[0].frames, 2);
        assert_eq!(rows[3].frames, 1);
        assert_eq!(rows[4].frames, 0);
        assert_eq!(rows.iter().map(|r| r.frames).sum::<u32>(), 3);
        assert!(account(&console.replace(" echo_dropped=1", ""), 12, 9).is_err());
    }
}
