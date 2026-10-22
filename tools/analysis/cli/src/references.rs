//! Human names of reference classifications.
use oer_analysis_blobray_application::references::Kind;

pub fn kind_human(kind: Kind) -> &'static str {
    match kind {
        Kind::Relocation => "relocation",
        Kind::Transfer => "transfer",
        Kind::TransferMay => "transfer-may",
        Kind::StoredWord => "stored-word",
        Kind::Materialized => "materialized",
    }
}
