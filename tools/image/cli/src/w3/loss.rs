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
    let bridge = number(console, "w3.echo ", "requests")?;
    let mut rows = pre_bridge(console, ap, bridge)?;
    rows.extend(
        stages([
            ap,
            bridge,
            number(console, "w3.stack ", "echo_requests")?,
            number(console, "w3.stack ", "echo_replies")?,
            number(console, "w3.stack ", "echo_refused")?,
            number(console, "w3.echo ", "replies")?,
            ap_replies,
        ])?
        .into_iter()
        .skip(1),
    );
    Ok(rows)
}

const REJECTIONS: [&str; 10] = [
    "header",
    "frame-kind",
    "address",
    "fragmented",
    "protection",
    "integrity",
    "replay",
    "payload",
    "not-connected",
    "security",
];
type Traffic = [u32; 8];

fn traffic(console: &str, stage: &str) -> Result<Traffic, String> {
    let mut lines = console
        .lines()
        .filter_map(|l| l.strip_prefix("w3.rx-traffic "))
        .filter(|l| {
            l.split_whitespace()
                .any(|w| w.strip_prefix("stage=") == Some(stage))
        });
    let line = lines
        .next()
        .ok_or_else(|| format!("missing RX stage {stage}"))?;
    if lines.next().is_some() {
        return Err(format!("repeated RX stage {stage}"));
    }
    let mut counts = [0; 8];
    for (count, key) in counts.iter_mut().zip([
        "request",
        "reply",
        "icmp",
        "ipv4",
        "arp",
        "ipv6",
        "other",
        "unidentified",
    ]) {
        let mut values = line
            .split_whitespace()
            .filter_map(|w| w.split_once('='))
            .filter(|(name, _)| *name == key);
        let (_, value) = values
            .next()
            .ok_or_else(|| format!("missing RX {stage} {key}"))?;
        if values.next().is_some() {
            return Err(format!("repeated RX {stage} {key}"));
        }
        *count = value
            .parse()
            .map_err(|_| format!("invalid RX {stage} {key}"))?;
        if *count == u32::MAX {
            return Err("saturated RX accounting".into());
        }
    }
    Ok(counts)
}
fn sum(rows: impl IntoIterator<Item = Traffic>) -> Result<Traffic, String> {
    let mut result = [0u32; 8];
    for row in rows {
        for (total, count) in result.iter_mut().zip(row) {
            *total = total.checked_add(count).ok_or("RX accounting overflow")?;
        }
    }
    Ok(result)
}
fn conserve(
    name: &str,
    input: Traffic,
    outputs: impl IntoIterator<Item = Traffic>,
) -> Result<(), String> {
    if input != sum(outputs)? {
        return Err(format!("RX boundary {name} does not conserve traffic"));
    }
    Ok(())
}

fn pre_bridge(console: &str, ap: u32, bridge: u32) -> Result<Vec<Row>, String> {
    let read = |name| traffic(console, name);
    let dma = read("dma")?;
    let staged = read("staged")?;
    let port = read("port")?;
    let service = read("service")?;
    let station = read("station")?;
    let queued = read("queue-admitted")?;
    let dma_drops = [read("dma-chained")?, read("dma-length")?, read("rx-pool")?];
    let port_drops = [
        read("prefix")?,
        read("decrypt-mic")?,
        read("filter-address")?,
    ];
    let service_drops = [read("reorder-duplicate")?, read("drain")?];
    let mut station_drops = vec![read("handled")?, read("queue-full")?, read("msdu-length")?];
    for name in REJECTIONS {
        station_drops.push(read(name)?);
    }
    conserve("DMA", dma, [staged].into_iter().chain(dma_drops))?;
    conserve("staging", staged, [port].into_iter().chain(port_drops))?;
    conserve("port", port, [service])?;
    conserve(
        "service",
        service,
        [station].into_iter().chain(service_drops),
    )?;
    conserve(
        "station",
        station,
        [queued].into_iter().chain(station_drops.iter().copied()),
    )?;
    if queued[0] != bridge {
        return Err("RX service/bridge echo scope differs".into());
    }
    // A damaged/aggregated frame with no recognizable IP identity could be
    // an AP request. Refuse to relabel such a software loss as an air loss.
    if dma_drops
        .into_iter()
        .chain(port_drops)
        .chain(service_drops)
        .chain(station_drops.iter().copied())
        .any(|row| row[7] != 0)
    {
        return Err("unidentified RX discard prevents pre-DMA ICMP attribution".into());
    }
    if number(console, "w3.rx-retry ", "request")? != 0 {
        return Err(
            "MAC Retry echoes require unique-request correlation before pre-DMA attribution".into(),
        );
    }
    if number(console, "w3.rx-retry ", "retired")? != 1
        || number(console, "w3.rx-retry ", "pending")? != 0
        || number(console, "w3.rx-retry ", "scoped")? != 1
    {
        return Err(
            "undrained or unscoped DMA observation prevents pre-DMA ICMP attribution".into(),
        );
    }
    let missing = ap
        .checked_sub(dma[0])
        .ok_or("RX DMA echo count exceeds AP requests")?;
    let mut rows = vec![Row {
        stage: "AP to RX DMA/ring (air/PHY; unassigned)",
        frames: missing,
    }];
    for name in [
        "dma-chained",
        "dma-length",
        "rx-pool",
        "prefix",
        "decrypt-mic",
        "filter-address",
        "reorder-duplicate",
        "drain",
        "handled",
        "queue-full",
        "msdu-length",
    ]
    .into_iter()
    .chain(REJECTIONS)
    {
        rows.push(Row {
            stage: name,
            frames: read(name)?[0],
        });
    }
    let lost = rows
        .iter()
        .try_fold(0u32, |n, r| n.checked_add(r.frames))
        .ok_or("RX sum overflow")?;
    if lost.checked_add(bridge) != Some(ap) {
        return Err("pre-bridge echoes do not conserve AP requests".into());
    }
    Ok(rows)
}
fn stages(
    [ap, bridge, stack, formed, refused, admitted, replies]: [u32; 7],
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
            stage: "Xarxa driver TX refusal",
            frames: refused,
        },
        Row {
            stage: "formed reply to service TX, excluding refusal",
            frames: diff(diff(formed, refused)?, admitted)?,
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
    fn receive_console() -> String {
        let mut console = String::new();
        for (stage, request, arp) in [
            ("dma", 7000, 2),
            ("staged", 6995, 2),
            ("dma-chained", 2, 0),
            ("dma-length", 1, 0),
            ("rx-pool", 2, 0),
            ("port", 6990, 2),
            ("prefix", 2, 0),
            ("decrypt-mic", 1, 0),
            ("filter-address", 2, 0),
            ("service", 6990, 2),
            ("station", 6990, 2),
            ("reorder-duplicate", 0, 0),
            ("drain", 0, 0),
            ("queue-admitted", 5000, 0),
            ("handled", 0, 0),
            ("queue-full", 1990, 2),
            ("msdu-length", 0, 0),
        ]
        .into_iter()
        .chain(REJECTIONS.map(|name| (name, 0, 0)))
        {
            console.push_str(&format!("w3.rx-traffic stage={stage} request={request} reply=0 icmp=0 ipv4=0 arp={arp} ipv6=0 other=0 unidentified=0\n"));
        }
        console.push_str("w3.rx-retry request=0 retired=1 pending=0 reserved=0 scoped=1\nw3.echo requests=5000 replies=3000\nw3.stack echo_requests=5000 echo_replies=5000 echo_refused=2000\n");
        console
    }
    #[test]
    fn dma_receipts_bound_the_air_residual_and_background_drops_are_conserved_separately() {
        let console = receive_console();
        let rows = account(&console, 10000, 3000).unwrap();
        assert!(account(&console.replace("pending=0", "pending=1"), 10000, 3000).is_err());
        assert!(account(&console.replace("scoped=1", "scoped=0"), 10000, 3000).is_err());
        assert_eq!(rows[0].frames, 3000);
        assert_eq!(
            rows.iter()
                .find(|r| r.stage == "queue-full")
                .unwrap()
                .frames,
            1990
        );
        assert_eq!(rows.iter().map(|r| r.frames).sum::<u32>() + 3000, 10000);
        assert!(
            account(
                &console.replace(
                    "request=1990 reply=0 icmp=0 ipv4=0 arp=2",
                    "request=1990 reply=0 icmp=0 ipv4=0 arp=3"
                ),
                10000,
                3000
            )
            .is_err()
        );
        assert!(
            account(
                &console.replace("w3.rx-retry request=0", "w3.rx-retry request=1"),
                10000,
                3000
            )
            .is_err()
        );
        assert!(
            account(
                &(console.clone() + "w3.rx-traffic stage=dma request=7000\n"),
                10000,
                3000
            )
            .is_err()
        );
        assert!(
            account(
                &console.replace("stage=dma request=7000", "stage=dma request=7001"),
                10000,
                3000
            )
            .is_err()
        );
    }
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
}
