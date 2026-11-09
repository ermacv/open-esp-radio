//! The probe's records, read from its console log. The grammar is the
//! probe's (`hil/probes/esp32s31-beacon-stamp/src/record.rs`); this reader
//! is written apart from it and refuses any line of that grammar it cannot
//! read whole. Other console lines are skipped.

/// The MAC local time before and after one AP TSF snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sample {
    pub local_before: u32,
    pub tsf: u64,
    pub local_after: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub at: Sample,
    pub status: u8,
    pub detail: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Blocker {
    pub published: Sample,
    pub done: Option<Completion>,
    pub queue: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Beacon,
    ProbeResponse,
    Action,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Beacon, Self::ProbeResponse, Self::Action];

    pub const fn code(self) -> u8 {
        match self {
            Self::Beacon => b'b',
            Self::ProbeResponse => b'p',
            Self::Action => b'a',
        }
    }

    /// Frame Control's first octet of this kind's frames.
    pub const fn frame_control(self) -> u8 {
        match self {
            Self::Beacon => 0x80,
            Self::ProbeResponse => 0x50,
            Self::Action => 0xd0,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Beacon => "beacon",
            Self::ProbeResponse => "probe-response",
            Self::Action => "action",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub n: u16,
    pub kind: Kind,
    pub delayed: bool,
    pub k: u64,
    pub blocker: Option<Blocker>,
    pub published: Sample,
    pub handed: Sample,
    pub done: Option<Completion>,
    pub readback: Option<[u8; 8]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub run: u32,
    pub channel: u8,
    pub ap: [u8; 6],
    pub absent: [u8; 6],
    pub blocker_body: usize,
    pub start: Option<Sample>,
    pub end: Option<Sample>,
    pub records: Vec<Record>,
    /// The probe's own end line: its record count, its error, if any, the
    /// MAC's RX buffer-full count, both stop results and the final check.
    pub reported: usize,
    pub error: Option<String>,
    pub rx_full: u32,
    pub stops: Stops,
}

/// How the probe's shared cleanup ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stops {
    pub mac: String,
    pub dma: String,
    pub finish: String,
}

impl Stops {
    /// The MAC went inactive, the walker stopped and the final check
    /// passed.
    pub fn confirmed(&self) -> bool {
        self.mac == "inactive" && self.dma == "stopped" && self.finish == "passed"
    }
}

fn field<'a>(line: &'a str, name: &str) -> Result<&'a str, String> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(name)?.strip_prefix('='))
        .ok_or_else(|| format!("no `{name}` in `{line}`"))
}

fn hex64(text: &str) -> Result<u64, String> {
    u64::from_str_radix(text, 16).map_err(|_| format!("`{text}` is not hexadecimal"))
}

fn hex32(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 16).map_err(|_| format!("`{text}` is not hexadecimal"))
}

fn hex8(text: &str) -> Result<u8, String> {
    u8::from_str_radix(text, 16).map_err(|_| format!("`{text}` is not hexadecimal"))
}

fn octets<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != N * 2 {
        return Err(format!("`{text}` is not {N} octets"));
    }
    let mut octets = [0u8; N];
    for (index, octet) in octets.iter_mut().enumerate() {
        *octet = hex8(text.get(index * 2..index * 2 + 2).ok_or("octets")?)?;
    }
    Ok(octets)
}

fn sample(text: &str) -> Result<Sample, String> {
    let parts: Vec<&str> = text.split(',').collect();
    let [before, tsf, after] = parts.as_slice() else {
        return Err(format!("`{text}` is not a sample"));
    };
    Ok(Sample {
        local_before: hex32(before)?,
        tsf: hex64(tsf)?,
        local_after: hex32(after)?,
    })
}

fn completion(text: &str) -> Result<Option<Completion>, String> {
    if text == "-" {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(',').collect();
    let [before, tsf, after, status, detail] = parts.as_slice() else {
        return Err(format!("`{text}` is not a completion"));
    };
    Ok(Some(Completion {
        at: sample(&format!("{before},{tsf},{after}"))?,
        status: hex8(status)?,
        detail: hex8(detail)?,
    }))
}

fn record(line: &str) -> Result<Record, String> {
    let kind = match field(line, "kind")? {
        "b" => Kind::Beacon,
        "p" => Kind::ProbeResponse,
        "a" => Kind::Action,
        other => return Err(format!("kind `{other}`")),
    };
    let delayed = match field(line, "arm")? {
        "u" => false,
        "d" => true,
        other => return Err(format!("arm `{other}`")),
    };
    let blocker = match field(line, "bpub")? {
        "-" => None,
        published => Some(Blocker {
            published: sample(published)?,
            done: completion(field(line, "bdone")?)?,
            queue: hex8(field(line, "bq")?)?,
        }),
    };
    if delayed != blocker.is_some() {
        return Err(format!("the arm and the blocker disagree in `{line}`"));
    }
    Ok(Record {
        n: field(line, "n")?
            .parse()
            .map_err(|_| format!("n in `{line}`"))?,
        kind,
        delayed,
        k: hex64(field(line, "k")?)?,
        blocker,
        published: sample(field(line, "pub")?)?,
        handed: sample(field(line, "hand")?)?,
        done: completion(field(line, "done")?)?,
        readback: match field(line, "rb")? {
            "-" => None,
            text => Some(octets::<8>(text)?),
        },
    })
}

/// The run in a console log: its begin line, clock lines, records and end
/// line, in that order.
pub fn parse(log: &str) -> Result<Run, String> {
    let mut run: Option<Run> = None;
    let mut ended = false;
    for line in log.lines() {
        let Some(start) = line.find("STAMP ") else {
            continue;
        };
        let line = line.get(start..).unwrap_or_default().trim_end();
        if line.starts_with("STAMP boot") || line.starts_with("STAMP previous") {
            continue;
        }
        if line.starts_with("STAMP v1 begin ") {
            if run.is_some() {
                return Err("two runs in one console log".into());
            }
            run = Some(Run {
                run: hex32(field(line, "run")?)?,
                channel: field(line, "channel")?
                    .parse()
                    .map_err(|_| "channel".to_owned())?,
                ap: octets::<6>(field(line, "ap")?)?,
                absent: octets::<6>(field(line, "absent")?)?,
                blocker_body: field(line, "blocker")?
                    .parse()
                    .map_err(|_| "blocker".to_owned())?,
                start: None,
                end: None,
                records: Vec::new(),
                reported: 0,
                error: None,
                rx_full: 0,
                stops: Stops {
                    mac: String::new(),
                    dma: String::new(),
                    finish: String::new(),
                },
            });
            continue;
        }
        let current = run
            .as_mut()
            .ok_or_else(|| format!("`{line}` before the begin line"))?;
        if ended {
            return Err(format!("`{line}` after the end line"));
        }
        if line.starts_with("STAMP clock ") {
            let value = sample(field(line, "s")?)?;
            match field(line, "at")? {
                "start" => current.start = Some(value),
                "end" => current.end = Some(value),
                other => return Err(format!("clock at `{other}`")),
            }
        } else if line.starts_with("STAMP rec ") {
            current.records.push(record(line)?);
        } else if line.starts_with("STAMP end ") {
            current.reported = field(line, "records")?
                .parse()
                .map_err(|_| "records".to_owned())?;
            current.error = match field(line, "error")? {
                "-" => None,
                error => Some(error.to_owned()),
            };
            current.rx_full = field(line, "rxfull")?
                .parse()
                .map_err(|_| "rxfull".to_owned())?;
            current.stops = Stops {
                mac: field(line, "mac")?.to_owned(),
                dma: field(line, "dma")?.to_owned(),
                finish: field(line, "finish")?.to_owned(),
            };
            ended = true;
        } else {
            return Err(format!("an unknown STAMP line `{line}`"));
        }
    }
    let run = run.ok_or("the console log has no STAMP begin line")?;
    if !ended {
        return Err("the console log has no STAMP end line".into());
    }
    if run.reported != run.records.len() {
        return Err(format!(
            "the probe reported {} records, the log holds {}",
            run.reported,
            run.records.len()
        ));
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "STAMP boot\n\
        STAMP v1 begin run=0badc0de channel=13 rounds=96 ap=6055f9010203 absent=024f45000001 blocker=1500 rate=dsss1\n\
        STAMP clock at=start s=001e8480,0000000000000005,001e8482\n\
        STAMP rec n=0 kind=b arm=u k=1122334455667788 bpub=- bdone=- bq=- pub=001e9000,0000000000000b85,001e9002 hand=001e9004,0000000000000b89,001e9006 done=001e9100,0000000000000c85,001e9102,00,00 rb=8877665544332211\n\
        STAMP rec n=1 kind=b arm=d k=2233445566778899 bpub=001f0000,0000000000016b85,001f0002 bdone=001f3080,0000000000019c05,001f3082,00,00 bq=03 pub=001f0400,0000000000016f85,001f0402 hand=001f0404,0000000000016f89,001f0406 done=001f3100,0000000000019c85,001f3102,00,00 rb=-\n\
        STAMP clock at=end s=002e8480,0000000000f42405,002e8482\n\
        STAMP end records=2 rxfull=0 error=- mac=inactive dma=stopped finish=passed\n\
        OER-RESULT: pass beacon-stamp\n";

    #[test]
    fn a_whole_run_is_read() {
        let run = parse(LOG).unwrap();
        assert_eq!(run.run, 0x0bad_c0de);
        assert_eq!(run.ap, [0x60, 0x55, 0xf9, 1, 2, 3]);
        assert_eq!(run.start.unwrap().local_before, 0x001e_8480);
        assert_eq!(run.rx_full, 0);
        assert!(run.stops.confirmed());
        assert_eq!(run.end.unwrap().tsf, 0xf4_2405);
        assert_eq!(run.records.len(), 2);
        let undelayed = run.records[0];
        assert!(!undelayed.delayed && undelayed.blocker.is_none());
        assert_eq!(
            undelayed.readback,
            Some(0x1122_3344_5566_7788u64.to_le_bytes())
        );
        let delayed = run.records[1];
        assert_eq!(delayed.blocker.unwrap().queue, 3);
        assert_eq!(delayed.done.unwrap().at.tsf, 0x19c85);
        assert_eq!(delayed.readback, None);
    }

    #[test]
    fn a_log_missing_its_end_or_a_record_is_refused() {
        let without_end: String = LOG
            .lines()
            .filter(|line| !line.starts_with("STAMP end"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(parse(&without_end).is_err());
        let without_record: String = LOG
            .lines()
            .filter(|line| !line.contains("n=1 "))
            .map(|line| format!("{line}\n"))
            .collect();
        assert!(parse(&without_record).unwrap_err().contains("reported 2"));
    }

    #[test]
    fn a_damaged_record_is_an_error_not_a_skipped_line() {
        let damaged = LOG.replace("bq=03", "bq=zz");
        assert!(parse(&damaged).is_err());
        let contradiction = LOG.replace("n=1 kind=b arm=d", "n=1 kind=b arm=u");
        assert!(parse(&contradiction).unwrap_err().contains("disagree"));
    }

    #[test]
    fn the_probes_error_rx_overflow_and_stops_are_kept() {
        let failed = LOG.replace(
            "rxfull=0 error=- mac=inactive dma=stopped finish=passed",
            "rxfull=7 error=completion-timeout mac=still-active dma=unconfirmed finish=not-run",
        );
        let run = parse(&failed).unwrap();
        assert_eq!(run.error.as_deref(), Some("completion-timeout"));
        assert_eq!(run.rx_full, 7);
        assert_eq!(run.stops.mac, "still-active");
        assert_eq!(run.stops.dma, "unconfirmed");
        assert!(!run.stops.confirmed());
    }

    #[test]
    fn an_end_line_without_its_stops_is_refused() {
        let old = LOG.replace(" mac=inactive dma=stopped finish=passed", "");
        assert!(parse(&old).is_err());
    }
}
