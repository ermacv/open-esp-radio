use super::*;

/// Little-endian words of 32-bit instructions.
fn code(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// `csrrci x0, mstatus, 8`: clears MIE.
const CLEAR_MIE: u32 = 0x3004_7073;
/// `csrrw x0, mintthresh, a0`: writes the threshold.
const WRITE_THRESHOLD: u32 = 0x3475_1073;
/// `csrrs a0, mstatus, x0`: reads only.
const READ_MSTATUS: u32 = 0x3000_2573;
/// `csrrs a0, mintthresh, x0`: reads only.
const READ_THRESHOLD: u32 = 0x3470_2573;
/// `csrrci x0, mstatus, 2`: clears SIE, not MIE.
const CLEAR_SIE: u32 = 0x3001_7073;
/// `addi x0, x0, 0`.
const NOP: u32 = 0x0000_0013;

#[test]
fn a_fixture_that_masks_in_two_functions_is_reported_by_function_and_address() {
    // Two functions: one clears MIE, one writes the threshold; each also
    // reads its CSR, which is no mask.
    let first = scan(
        0x4000_0000,
        &code(&[NOP, READ_MSTATUS, CLEAR_MIE, CLEAR_SIE]),
    );
    let second = scan(0x4000_0100, &code(&[READ_THRESHOLD, WRITE_THRESHOLD]));
    assert_eq!(
        first
            .iter()
            .map(|site| (site.address, site.mask))
            .collect::<Vec<_>>(),
        [(0x4000_0008, Mask::ClearsMie)]
    );
    assert_eq!(
        second
            .iter()
            .map(|site| (site.address, site.mask))
            .collect::<Vec<_>>(),
        [(0x4000_0104, Mask::WritesThreshold)]
    );
    let rows: Vec<_> = first
        .into_iter()
        .map(|site| (site, "masks".to_owned()))
        .chain(
            second
                .into_iter()
                .map(|site| (site, "thresholds".to_owned())),
        )
        .collect();
    let report = render(&rows);
    assert!(report.contains("0x40000008\t"), "{report}");
    assert!(report.contains("clears mstatus.MIE\tmasks"), "{report}");
    assert!(
        report.contains("writes the threshold\tthresholds"),
        "{report}"
    );
    assert_eq!(report.lines().count(), 4, "{report}");
}

#[test]
fn an_image_without_a_mask_says_so() {
    assert!(scan(0, &code(&[NOP, READ_MSTATUS, READ_THRESHOLD])).is_empty());
    assert!(render(&[]).ends_with("none\n"));
}
