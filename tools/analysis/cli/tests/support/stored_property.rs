use oer_analysis_blobray_domain::{ReadUses, StoredBits, StoredBitsSource};

pub fn identity_bits(stored: &[StoredBits]) -> u32 {
    stored.iter().fold(0, |bits, run| {
        if matches!(run.source, StoredBitsSource::Load { same_word: true, low, .. } if low == run.low) {
            bits | (u32::MAX >> (32 - u32::from(run.width))) << run.low
        } else {
            bits
        }
    })
}

fn unknown_bits(stored: &[StoredBits]) -> u32 {
    stored.iter().fold(0, |bits, run| {
        if run.source == StoredBitsSource::Unknown {
            bits | (u32::MAX >> (32 - u32::from(run.width))) << run.low
        } else {
            bits
        }
    })
}

/// Bits independently proved to join different tags at this store. Unknown
/// from inversion, unsupported operations or a limit is not an exception.
pub struct JoinedBits<'a> {
    pub function: &'a [u8],
    pub store: u64,
    pub bits: u32,
}

/// Pair source and sink before calling: schema 9 omits load-site ids. The
/// caller proves joined_bits from the fixture's paths, not from Unknown alone.
pub fn missing_preserved_bits(reads: &ReadUses, stored: &[StoredBits], joined_bits: u32) -> u32 {
    reads.preserved & !(identity_bits(stored) | (joined_bits & unknown_bits(stored)))
}

/// Every preserved source bit has a same-word, same-position store witness,
/// except a bit proved to join different tags at the store.
/// The public schema omits load IDs, so combine witnesses for the matching
/// function, abstract address and access width.
pub fn assert_preserved_stores(
    observations: &[oer_analysis_blobray_domain::RegisterAccess],
    joins: &[JoinedBits<'_>],
) {
    use oer_analysis_blobray_domain::RegisterAccess;
    use oer_analysis_riscv_model::{FunctionRecord, MemoryKind};
    for join in joins {
        let mut found = false;
        for observation in observations {
            if let RegisterAccess::Observation {
                function,
                fact,
                stored: Some(runs),
                ..
            } = observation
                && function.name.as_deref() == Some(join.function)
                && matches!(fact.as_ref(), FunctionRecord::MemoryAccess { offset, access: MemoryKind::Store, .. } if *offset == join.store)
            {
                assert_eq!(
                    join.bits & !unknown_bits(runs),
                    0,
                    "joined bits must be Unknown"
                );
                found = true;
            }
        }
        assert!(found, "a join exception needs its store");
    }
    for observation in observations {
        let RegisterAccess::Observation {
            function,
            fact,
            reads: Some(reads),
            address: resolved,
            indexed,
            ..
        } = observation
        else {
            continue;
        };
        if reads.preserved == 0 || (resolved.is_none() && indexed.is_none()) {
            continue;
        }
        let FunctionRecord::MemoryAccess { address, width, .. } = fact.as_ref() else {
            continue;
        };
        let mut stored = 0;
        for candidate in observations {
            if let RegisterAccess::Observation {
                function: target,
                fact,
                stored: Some(runs),
                ..
            } = candidate
                && target == function
                && matches!(fact.as_ref(), FunctionRecord::MemoryAccess { access: MemoryKind::Store, address: a, width: w, .. } if a == address && w == width)
            {
                stored |= identity_bits(runs);
                if let FunctionRecord::MemoryAccess { offset, .. } = fact.as_ref() {
                    let joined = joins
                        .iter()
                        .filter(|join| {
                            function.name.as_deref() == Some(join.function) && join.store == *offset
                        })
                        .fold(0, |bits, join| bits | join.bits);
                    stored |= joined & unknown_bits(runs);
                }
            }
        }
        assert_eq!(
            reads.preserved & !stored,
            0,
            "preserved bits need stored witnesses or proved tag joins: {observation:?}"
        );
    }
}
