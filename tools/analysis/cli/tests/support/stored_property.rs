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

/// Property for a load whose only sink is this same-word store. Pair the
/// source and sink before calling: schema 9 stored runs omit load-site ids.
pub fn missing_preserved_bits(reads: &ReadUses, stored: &[StoredBits]) -> u32 {
    reads.preserved & !identity_bits(stored)
}

/// Every preserved source bit has a same-word, same-position store witness.
/// The public schema omits load IDs, so combine witnesses for the matching
/// function, abstract address and access width.
pub fn assert_preserved_stores(observations: &[oer_analysis_blobray_domain::RegisterAccess]) {
    use oer_analysis_blobray_domain::RegisterAccess;
    use oer_analysis_riscv_model::{FunctionRecord, MemoryKind};
    for observation in observations {
        let RegisterAccess::Observation {
            function,
            fact,
            reads: Some(reads),
            ..
        } = observation
        else {
            continue;
        };
        if reads.preserved == 0 {
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
            }
        }
        assert_eq!(
            reads.preserved & !stored,
            0,
            "preserved bits need stored witnesses: {observation:?}"
        );
    }
}
