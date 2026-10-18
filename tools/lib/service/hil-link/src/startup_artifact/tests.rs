use super::*;

#[test]
fn chunk_assembly_preserves_non_aligned_artifact() {
    let bytes = (0..777).map(|index| index as u8).collect::<Vec<_>>();
    let chunks = chunks(&bytes).unwrap();
    assert_eq!(
        chunks.len(),
        bytes.len().div_ceil(STARTUP_ARTIFACT_CHUNK_MAX_LEN)
    );
    let mut assembler = Assembler::new();
    let mut completed = None;
    for chunk in &chunks {
        completed = assembler.push(chunk).unwrap().or(completed);
    }
    assert_eq!(completed.as_deref(), Some(bytes.as_slice()));
}

#[test]
fn assembly_rejects_missing_middle_chunk() {
    let bytes = vec![0xa5; 900];
    let chunks = chunks(&bytes).unwrap();
    let mut assembler = Assembler::new();
    assert!(assembler.push(&chunks[0]).unwrap().is_none());
    assert!(assembler.push(&chunks[2]).is_err());
}

#[test]
fn assembly_rejects_complete_artifact_with_wrong_digest() {
    let bytes = vec![0x3c; 500];
    let wrong_checksum = startup_artifact_crc32c(&bytes) ^ 1;
    let mut assembler = Assembler::new();
    for (index, part) in bytes.chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN).enumerate() {
        let chunk = StartupArtifactChunk::try_new(
            500,
            u16::try_from(index * STARTUP_ARTIFACT_CHUNK_MAX_LEN).unwrap(),
            wrong_checksum,
            part,
        )
        .unwrap();
        if chunk.is_final() {
            assert!(assembler.push(&chunk).is_err());
        } else {
            assert!(assembler.push(&chunk).unwrap().is_none());
        }
    }
}
