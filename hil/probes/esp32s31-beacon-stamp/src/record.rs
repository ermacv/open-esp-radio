//! What the device keeps of each round, and the console lines that report
//! it after the run. The host program (`hil/scenarios/beacon-stamp`) reads
//! these lines; their grammar is its input format:
//!
//! ```text
//! STAMP v1 begin run=<8 hex> channel=<dec> rounds=<dec> ap=<12 hex> absent=<12 hex> blocker=<dec> rate=dsss1
//! STAMP clock at=<start|end> s=<sample>
//! STAMP rec n=<dec> kind=<b|p|a> arm=<u|d> k=<16 hex> bpub=<sample|-> bdone=<completion|-> bq=<2 hex|-> pub=<sample> hand=<sample> done=<completion|-> rb=<16 hex|->
//! STAMP end records=<dec> rxfull=<dec> error=<name|->
//! ```
//!
//! A `<sample>` is `<local before>,<AP TSF>,<local after>` in hexadecimal
//! (8, 16 and 8 digits): the MAC local time read before and after one AP
//! TSF snapshot. A `<completion>` is `<sample>,<status>,<detail>`, the
//! sample taken when the completion was observed. `rb` is octets 24..32 of
//! the subject MPDU read back from its transmit buffer after completion, in
//! buffer order.

use crate::frames::{Arm, Kind};
use crate::line::{Line, LineFull};

/// The MAC local time before and after one AP TSF snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Sample {
    pub(crate) local_before: u32,
    pub(crate) tsf: u64,
    pub(crate) local_after: u32,
}

/// An observed completion: when it was seen and what the hardware said.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Completion {
    pub(crate) at: Sample,
    pub(crate) status: u8,
    pub(crate) detail: u8,
}

/// The blocker of a delayed round.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Blocker {
    pub(crate) published: Sample,
    pub(crate) done: Option<Completion>,
    /// The blocker queue's state when the subject was published: bit 0
    /// valid, bit 1 enabled, bit 2 completion pending.
    pub(crate) queue: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Record {
    pub(crate) n: u16,
    pub(crate) kind: Kind,
    pub(crate) arm: Arm,
    pub(crate) k: u64,
    pub(crate) blocker: Option<Blocker>,
    /// Taken immediately before the subject's queue publication.
    pub(crate) published: Sample,
    /// Taken immediately after it returned.
    pub(crate) handed: Sample,
    pub(crate) done: Option<Completion>,
    pub(crate) readback: Option<[u8; 8]>,
}

impl Record {
    pub(crate) const EMPTY: Self = Self {
        n: 0,
        kind: Kind::Beacon,
        arm: Arm::Undelayed,
        k: 0,
        blocker: None,
        published: Sample {
            local_before: 0,
            tsf: 0,
            local_after: 0,
        },
        handed: Sample {
            local_before: 0,
            tsf: 0,
            local_after: 0,
        },
        done: None,
        readback: None,
    };
}

/// A record line's capacity: every field at its widest.
pub(crate) const RECORD_LINE: usize = 320;

fn sample<const N: usize>(line: &mut Line<N>, sample: Sample) -> Result<(), LineFull> {
    line.hex32(sample.local_before)?
        .text(b",")?
        .hex64(sample.tsf)?
        .text(b",")?
        .hex32(sample.local_after)?;
    Ok(())
}

fn completion<const N: usize>(
    line: &mut Line<N>,
    completion: Option<Completion>,
) -> Result<(), LineFull> {
    match completion {
        None => {
            line.text(b"-")?;
        }
        Some(done) => {
            sample(line, done.at)?;
            line.text(b",")?
                .hex8(done.status)?
                .text(b",")?
                .hex8(done.detail)?;
        }
    }
    Ok(())
}

pub(crate) fn record_line(record: &Record) -> Result<Line<RECORD_LINE>, LineFull> {
    let mut line = Line::new();
    line.text(b"STAMP rec n=")?
        .decimal(u64::from(record.n))?
        .text(b" kind=")?
        .text(&[record.kind.code()])?
        .text(b" arm=")?
        .text(&[record.arm.code()])?
        .text(b" k=")?
        .hex64(record.k)?
        .text(b" bpub=")?;
    match record.blocker {
        None => {
            line.text(b"- bdone=- bq=-")?;
        }
        Some(blocker) => {
            sample(&mut line, blocker.published)?;
            line.text(b" bdone=")?;
            completion(&mut line, blocker.done)?;
            line.text(b" bq=")?.hex8(blocker.queue)?;
        }
    }
    line.text(b" pub=")?;
    sample(&mut line, record.published)?;
    line.text(b" hand=")?;
    sample(&mut line, record.handed)?;
    line.text(b" done=")?;
    completion(&mut line, record.done)?;
    line.text(b" rb=")?;
    match record.readback {
        None => line.text(b"-")?,
        Some(octets) => line.hex_bytes(&octets)?,
    };
    line.text(b"\n")?;
    Ok(line)
}

pub(crate) fn clock_line(at: &[u8], value: Sample) -> Result<Line<64>, LineFull> {
    let mut line = Line::new();
    line.text(b"STAMP clock at=")?.text(at)?.text(b" s=")?;
    sample(&mut line, value)?;
    line.text(b"\n")?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Sample = Sample {
        local_before: 0x0030_0000,
        tsf: 0x10,
        local_after: 0x0030_0002,
    };

    #[test]
    fn an_undelayed_record_without_a_completion_says_so() {
        let record = Record {
            n: 2,
            k: 0x1122_3344_5566_7788,
            published: S,
            handed: S,
            ..Record::EMPTY
        };
        let line = record_line(&record).unwrap();
        assert_eq!(
            core::str::from_utf8(line.as_bytes()).unwrap(),
            "STAMP rec n=2 kind=b arm=u k=1122334455667788 bpub=- bdone=- bq=- \
             pub=00300000,0000000000000010,00300002 hand=00300000,0000000000000010,00300002 \
             done=- rb=-\n"
        );
    }

    #[test]
    fn the_widest_delayed_record_fits_its_line() {
        let wide = Sample {
            local_before: u32::MAX,
            tsf: u64::MAX,
            local_after: u32::MAX,
        };
        let done = Completion {
            at: wide,
            status: 0xff,
            detail: 0xff,
        };
        let record = Record {
            n: u16::MAX,
            kind: Kind::ProbeResponse,
            arm: Arm::Delayed,
            k: u64::MAX,
            blocker: Some(Blocker {
                published: wide,
                done: Some(done),
                queue: 7,
            }),
            published: wide,
            handed: wide,
            done: Some(done),
            readback: Some([0xab; 8]),
        };
        let line = record_line(&record).unwrap();
        let text = core::str::from_utf8(line.as_bytes()).unwrap();
        assert!(text.contains(" bq=07 pub="), "{text}");
        assert!(text.ends_with(",ff,ff rb=abababababababab\n"), "{text}");
        assert!(text.starts_with("STAMP rec n=65535 kind=p arm=d k=ffffffffffffffff bpub="));
    }

    #[test]
    fn a_clock_line_names_its_point() {
        let line = clock_line(b"start", S).unwrap();
        assert_eq!(
            line.as_bytes(),
            b"STAMP clock at=start s=00300000,0000000000000010,00300002\n"
        );
    }
}
