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
