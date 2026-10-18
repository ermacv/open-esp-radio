//! Strict interpretation of retained execution evidence.
//!
//! Every helper fails instead of guessing: unknown, unavailable or
//! noncontiguous bytes and unknown call words are never read as values.
use blobray_domain::{
    ExecutionEvent, ExecutionEvidence, ExecutionStop, ObservedCallTarget, ObservedWord,
};

/// All selected bytes of the first final-memory selection of one case side.
pub fn output(records: &[ExecutionEvidence], case: u32, side: bool) -> Vec<u8> {
    selection(records, case, side, 0)
}

/// All bytes of final-memory selection `index` of one case side.
pub fn selection(records: &[ExecutionEvidence], case: u32, side: bool, index: u16) -> Vec<u8> {
    let chunks: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::FinalMemory {
                case: c,
                replacement,
                chunk,
            } if *c == case && *replacement == side && chunk.selection == index => Some(chunk),
            _ => None,
        })
        .collect();
    assert!(
        !chunks.is_empty(),
        "case {case} side {side} has no final memory in selection {index}"
    );
    let mut offset = 0;
    let mut bytes = vec![];
    for chunk in chunks {
        assert!(
            chunk.offset == offset,
            "noncontiguous final memory in case {case} selection {index}"
        );
        let mask = chunk.mask().expect("valid chunk length");
        assert!(
            chunk.known == mask && chunk.available == mask,
            "unknown or unavailable final memory in case {case} selection {index}"
        );
        bytes.extend_from_slice(&chunk.bytes[..usize::from(chunk.length)]);
        offset += u32::from(chunk.length);
    }
    bytes
}

/// Executed instruction steps of one case side.
pub fn steps(records: &[ExecutionEvidence], case: u32, side: bool) -> u64 {
    records
        .iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case: c,
                replacement,
                steps,
                ..
            } if *c == case && *replacement == side => Some(*steps),
            _ => None,
        })
        .unwrap_or_else(|| panic!("case {case} side {side} has no outcome"))
}

/// Ordered events of one case side.
pub fn events(records: &[ExecutionEvidence], case: u32, side: bool) -> Vec<ExecutionEvent> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                case: c,
                replacement,
                event,
            } if *c == case && *replacement == side => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Terminal stop of one case side.
pub fn stop(records: &[ExecutionEvidence], case: u32, side: bool) -> ExecutionStop {
    records
        .iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case: c,
                replacement,
                stop,
                ..
            } if *c == case && *replacement == side => Some(stop.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("case {case} side {side} has no outcome"))
}

pub fn outcomes(records: &[ExecutionEvidence]) -> Vec<&ExecutionStop> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Outcome { stop, .. } => Some(stop),
            _ => None,
        })
        .collect()
}

pub fn has_events(records: &[ExecutionEvidence]) -> bool {
    records
        .iter()
        .any(|r| matches!(r, ExecutionEvidence::Event { .. }))
}

/// Known argument words of each captured-code transfer to `target`.
pub fn calls(observations: &[ExecutionEvent], target: u32) -> Vec<Vec<u32>> {
    let mut found = vec![];
    for (index, event) in observations.iter().enumerate() {
        let ExecutionEvent::CallTransfer {
            target: t,
            target_kind,
            words,
            ..
        } = event
        else {
            continue;
        };
        if *t != target {
            continue;
        }
        assert_eq!(
            *target_kind,
            ObservedCallTarget::CapturedCode,
            "call to {target:#x} is not captured code"
        );
        let arguments = &observations[index + 1..];
        let words = usize::from(*words);
        assert!(arguments.len() >= words, "missing call argument records");
        found.push(
            arguments[..words]
                .iter()
                .map(|a| match a {
                    ExecutionEvent::TransferArgument {
                        value: ObservedWord::Known { value },
                        ..
                    } => *value,
                    other => panic!("call argument is not a known word: {other:?}"),
                })
                .collect(),
        );
    }
    found
}

/// Argument words of each modeled call to `target`, by word index: `None`
/// for a word the call did not capture or captured as unknown.
pub fn modeled_calls(observations: &[ExecutionEvent], target: u32) -> Vec<Vec<Option<u32>>> {
    let mut found: Vec<Vec<Option<u32>>> = vec![];
    let mut current = false;
    for event in observations {
        match event {
            ExecutionEvent::ModeledCall { target: t, .. } => {
                current = *t == target;
                if current {
                    found.push(vec![]);
                }
            }
            ExecutionEvent::CallArgument { word, value } if current => {
                let words = found.last_mut().expect("an open modeled call");
                let word = usize::from(*word);
                if words.len() <= word {
                    words.resize(word + 1, None);
                }
                words[word] = *value;
            }
            ExecutionEvent::CallArgument { .. } => {}
            _ => current = false,
        }
    }
    found
}

/// Kind, address and value of an MMIO transaction, for ordered comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read,
    Write,
}
pub type Effect = (Access, u32, u32);

/// MMIO read/write effects, all of which must be word-sized. Other event kinds
/// in `observed` must be listed in `ignored` or the helper fails.
pub fn effects(observed: &[ExecutionEvent], ignore_calls: bool) -> Vec<Effect> {
    observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read {
                address,
                width,
                value,
            } => {
                assert_eq!(*width, 4, "non-word MMIO read");
                Some((Access::Read, *address, *value))
            }
            ExecutionEvent::Write {
                address,
                width,
                value,
            } => {
                assert_eq!(*width, 4, "non-word MMIO write");
                Some((Access::Write, *address, *value))
            }
            ExecutionEvent::CallTransfer { .. } | ExecutionEvent::TransferArgument { .. }
                if ignore_calls =>
            {
                None
            }
            other => panic!("unexpected event {other:?}"),
        })
        .collect()
}

/// One ordered word MMIO effect or requested delay of one side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyEffect {
    Read(u32, u32),
    Write(u32, u32),
    Delay(u32),
}

/// Ordered word MMIO effects and requested delays of one side, for
/// single-side characterization. Blobray compares the two sides under the
/// reviewed effect contract. Non-word MMIO fails.
pub fn phy_effects(observed: &[ExecutionEvent]) -> Vec<PhyEffect> {
    observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read {
                address,
                width: 4,
                value,
            } => Some(PhyEffect::Read(*address, *value)),
            ExecutionEvent::Write {
                address,
                width: 4,
                value,
            } => Some(PhyEffect::Write(*address, *value)),
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. } => {
                panic!("non-word PHY MMIO {event:?}")
            }
            ExecutionEvent::DelayMicros { value } => Some(PhyEffect::Delay(*value)),
            _ => None,
        })
        .collect()
}

/// Case number of a per-case record; coverage spans the whole request.
fn record_case(record: &ExecutionEvidence) -> Option<u32> {
    match record {
        ExecutionEvidence::FinalMemory { case, .. }
        | ExecutionEvidence::CallModel { case, .. }
        | ExecutionEvidence::Model { case, .. }
        | ExecutionEvidence::Event { case, .. }
        | ExecutionEvidence::Written { case, .. }
        | ExecutionEvidence::Outcome { case, .. }
        | ExecutionEvidence::Comparison { case, .. } => Some(*case),
        ExecutionEvidence::Coverage { .. } => None,
    }
}

/// The contiguous records of each case, keeping their case numbers, so a
/// check of one case reads only its own records. Retained records are
/// ordered by case.
pub fn case_slices(
    records: &[ExecutionEvidence],
) -> std::collections::BTreeMap<u32, &[ExecutionEvidence]> {
    let mut slices = std::collections::BTreeMap::new();
    let mut start = 0;
    while start < records.len() {
        let Some(case) = record_case(&records[start]) else {
            start += 1;
            continue;
        };
        let end = start
            + records[start..]
                .iter()
                .take_while(|r| record_case(r) == Some(case))
                .count();
        assert!(
            slices.insert(case, &records[start..end]).is_none(),
            "records of case {case} are not contiguous"
        );
        start = end;
    }
    slices
}

/// Records of consecutive case ranges of one request, each renumbered from
/// zero: the first `counts[0]` cases form part 0, and so on. Coverage records
/// span the whole request and are omitted. Checks written
/// for one profile's request then apply unchanged to a merged request.
pub fn split_cases(records: &[ExecutionEvidence], counts: &[u32]) -> Vec<Vec<ExecutionEvidence>> {
    let mut starts = vec![0u32];
    for count in counts {
        starts.push(starts.last().unwrap() + count);
    }
    let mut parts = vec![vec![]; counts.len()];
    for record in records {
        let mut record = record.clone();
        let case = match &mut record {
            ExecutionEvidence::FinalMemory { case, .. }
            | ExecutionEvidence::CallModel { case, .. }
            | ExecutionEvidence::Model { case, .. }
            | ExecutionEvidence::Event { case, .. }
            | ExecutionEvidence::Written { case, .. }
            | ExecutionEvidence::Outcome { case, .. }
            | ExecutionEvidence::Comparison { case, .. } => case,
            // Whole-execution coverage belongs to no single part.
            ExecutionEvidence::Coverage { .. } => continue,
        };
        let part = starts
            .windows(2)
            .position(|w| (w[0]..w[1]).contains(case))
            .expect("record case outside the declared parts");
        *case -= starts[part];
        parts[part].push(record);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::FinalMemoryChunk;

    fn chunk(offset: u32, known: u16, available: u16, selection: u16) -> ExecutionEvidence {
        let mut bytes = [0; 16];
        bytes[..2].copy_from_slice(&[0x12, 0x34]);
        ExecutionEvidence::FinalMemory {
            case: 1,
            replacement: false,
            chunk: FinalMemoryChunk {
                selection,
                offset,
                length: 2,
                bytes,
                available,
                known,
            },
        }
    }

    #[test]
    fn split_cases_renumbers_each_part_from_zero() {
        let records = [chunk(0, 3, 3, 0), chunk(0, 3, 3, 0), chunk(0, 3, 3, 0)];
        let mut records = records.to_vec();
        for (i, r) in records.iter_mut().enumerate() {
            if let ExecutionEvidence::FinalMemory { case, .. } = r {
                *case = [0, 2, 3][i];
            }
        }
        let parts = split_cases(&records, &[2, 2]);
        let cases = |part: &[ExecutionEvidence]| -> Vec<u32> {
            part.iter()
                .map(|r| match r {
                    ExecutionEvidence::FinalMemory { case, .. } => *case,
                    _ => unreachable!(),
                })
                .collect()
        };
        assert_eq!(cases(&parts[0]), [0]);
        assert_eq!(cases(&parts[1]), [0, 1]);
        assert!(std::panic::catch_unwind(|| split_cases(&records, &[1])).is_err());
    }

    #[test]
    fn output_reads_known_contiguous_bytes() {
        assert_eq!(output(&[chunk(0, 3, 3, 0)], 1, false), [0x12, 0x34]);
    }

    #[test]
    fn output_rejects_unknown_unavailable_and_noncontiguous_bytes() {
        for record in [
            chunk(0, 1, 3, 0),
            chunk(0, 3, 1, 0),
            chunk(1, 3, 3, 0),
            chunk(0, 3, 3, 1),
        ] {
            let result = std::panic::catch_unwind(|| output(&[record], 1, false));
            assert!(result.is_err());
        }
        assert!(
            std::panic::catch_unwind(|| output(&[chunk(0, 3, 3, 0), chunk(0, 3, 3, 0)], 1, false))
                .is_err()
        );
        assert!(std::panic::catch_unwind(|| output(&[chunk(0, 3, 3, 0)], 2, false)).is_err());
    }

    #[test]
    fn phy_effects_keep_order_and_reject_narrow_mmio() {
        let observed = [
            ExecutionEvent::Write {
                address: 8,
                width: 4,
                value: 1,
            },
            ExecutionEvent::DelayMicros { value: 1 },
            ExecutionEvent::Read {
                address: 8,
                width: 4,
                value: 2,
            },
        ];
        assert_eq!(
            phy_effects(&observed),
            [
                PhyEffect::Write(8, 1),
                PhyEffect::Delay(1),
                PhyEffect::Read(8, 2)
            ]
        );
        let narrow = [ExecutionEvent::Read {
            address: 8,
            width: 1,
            value: 2,
        }];
        assert!(std::panic::catch_unwind(|| phy_effects(&narrow)).is_err());
    }

    fn transfer(kind: ObservedCallTarget) -> ExecutionEvent {
        ExecutionEvent::CallTransfer {
            site: 0,
            target: 10,
            tail: false,
            indirect: false,
            stack: None,
            target_kind: kind,
            words: 1,
        }
    }

    #[test]
    fn calls_reject_models_and_unknown_arguments() {
        let known = ExecutionEvent::TransferArgument {
            word: 0,
            value: ObservedWord::Known { value: 123 },
        };
        assert_eq!(
            calls(
                &[transfer(ObservedCallTarget::CapturedCode), known.clone()],
                10
            ),
            [[123]]
        );
        let unknown = ExecutionEvent::TransferArgument {
            word: 0,
            value: ObservedWord::Unknown,
        };
        for events in [
            vec![transfer(ObservedCallTarget::CallModel), known],
            vec![transfer(ObservedCallTarget::CapturedCode), unknown],
            vec![transfer(ObservedCallTarget::CapturedCode)],
        ] {
            assert!(std::panic::catch_unwind(|| calls(&events, 10)).is_err());
        }
    }
}
