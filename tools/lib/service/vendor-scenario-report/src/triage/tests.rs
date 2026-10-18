use super::*;
use crate::inspect::Corpus;

/// `lui a5, 0x20104`; `lw a4, 32(a5)`; `andi a4, a4, 1`;
/// `beqz a4, +8`; `jal ra, +8`; `sw a0, 12(sp)`; `ret`; `ret`;
/// `sw a0, 0(a5)`; `ret`.
const CODE: [u32; 10] = [
    0x2010_47b7,
    0x0207_a703,
    0x0017_7713,
    0x0007_0463,
    0x0080_00ef,
    0x00a1_2623,
    0x0000_8067,
    0x0000_8067,
    0x00a7_a023,
    0x0000_8067,
];
/// Where the probe starts and where its callee, a log function, sits.
const PROBE: u32 = 0x4000_0000;
const LOG: u32 = 0x4000_0018;
/// `ret`, the log function's body.
const RET: [u8; 4] = 0x0000_8067u32.to_le_bytes();

fn code(diagnostic: &[&str]) -> Code {
    let bytes: Vec<u8> = CODE.iter().flat_map(|w| w.to_le_bytes()).collect();
    let registers = Registers::parse(
        "schema = 2\ncrate-name = \"oer_chip_pac_raw\"\n\
         [[registers]]\naddress = 0x20104020\nwidth = 32\naccess = \"read-write\"\n\
         identity = \"MAC.TX_CONFIG\"\nperipheral = \"MAC\"\nperipheral-type = \"Mac\"\n\
         peripheral-module = \"mac\"\nregister-method = \"tx_config\"\n\
         [[registers.fields]]\nsvd-name = \"ENABLE\"\nmethod = \"enable\"\nbit-offset = 0\n\
         bit-width = 1\naccess = \"read-write\"\n",
    )
    .unwrap();
    Code {
        corpus: Corpus::linked(
            &[("probe", PROBE, &bytes), ("wifi_log", LOG, &RET)],
            registers,
        ),
        diagnostic: diagnostic.iter().map(|d| (*d).to_owned()).collect(),
    }
}

#[test]
fn constants_fold_into_named_registers_and_fields() {
    let lines = code(&[]).lines("probe").unwrap();
    let text = |index: usize| lines[index].display();
    assert!(text(0).ends_with("; = 0x20104000"), "{}", text(0));
    assert!(
        text(1).ends_with("; [0x20104020 MAC.TX_CONFIG]"),
        "{}",
        text(1)
    );
    assert!(text(2).ends_with("; MAC.TX_CONFIG: ENABLE"), "{}", text(2));
    let found = definitions(&lines, 3);
    assert!(found[0].starts_with("a4 <- +0x8: "), "{found:?}");
    assert!(
        found.iter().any(|f| f.starts_with("a4 <- +0x4: ")),
        "{found:?}"
    );
}

#[test]
fn a_block_ends_at_its_first_transfer_other_than_a_call() {
    let lines = code(&[]).lines("probe").unwrap();
    // The call at +0x10 continues the block; the return at +0x18 ends it.
    assert_eq!(block_end(&lines, 4), 6);
    assert_eq!(block_end(&lines, 0), 3);
}

#[test]
fn only_reviewed_diagnostic_calls_make_a_candidate() {
    let unreviewed = code(&[]);
    let lines = unreviewed.lines("probe").unwrap();
    assert_eq!(lines[4].callee.as_deref(), Some("wifi_log"));
    // A callee no decision names as diagnostic output is never one.
    assert_eq!(unreviewed.diagnostic_only(&lines, 4), None);
    let reviewed = code(&["wifi_log"]);
    let lines = reviewed.lines("probe").unwrap();
    assert_eq!(reviewed.diagnostic_only(&lines, 4), Some(Ending::Returns));
    // A store outside the stack is never diagnostic.
    assert_eq!(reviewed.diagnostic_only(&lines, 8), None);
}

#[test]
fn an_assertion_and_a_late_read_are_told_apart() {
    // jal ra, +8 (to the log); j . (spin): an assertion.
    let spin = [0x0080_00ef_u32, 0x0000_006f];
    // jal ra, +8; lw a5, 4(a0); j .: a read of object state after the log.
    let late = [0x0080_00ef_u32, 0x0045_2783, 0x0000_006f];
    // lw a5, 4(a0); jal ra, +8; j .: the read comes before the output.
    let early = [0x0045_2783_u32, 0x0080_00ef, 0x0000_006f];
    let check = |words: &[u32], log: u32| {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let code = Code {
            corpus: Corpus::linked(
                &[("probe", PROBE, &bytes), ("wifi_log", PROBE + log, &RET)],
                Registers::default(),
            ),
            diagnostic: BTreeSet::from(["wifi_log".to_owned()]),
        };
        let lines = code.lines("probe").unwrap();
        code.diagnostic_only(&lines, 0)
    };
    assert_eq!(check(&spin, 8), Some(Ending::Spins));
    assert_eq!(check(&late, 8), None);
    assert_eq!(check(&early, 12), Some(Ending::Spins));
}

#[test]
fn the_function_view_marks_every_location_by_its_triage() {
    let at = |offset, kind| Location {
        function: "probe".into(),
        offset,
        kind,
    };
    let untriaged = BTreeSet::from([at(0xc, LocationKind::Taken)]);
    let uncovered = BTreeSet::from([
        at(0xc, LocationKind::Taken),
        at(0x10, LocationKind::Block),
        at(0x20, LocationKind::Block),
    ]);
    let consequential = BTreeSet::from([at(0x20, LocationKind::Block)]);
    let view = functions(&code(&[]), &untriaged, &uncovered, &consequential, |l| {
        (l.offset == 0x10).then_some("reviewed path")
    });
    let line = |offset: &str| {
        view.lines()
            .find(|l| l.contains(offset))
            .unwrap()
            .to_owned()
    };
    assert!(line("+000c").contains("U taken"), "{view}");
    assert!(line("+0010").contains("E1 block"), "{view}");
    assert!(line("+0020").contains("C block"), "{view}");
    assert!(!line("+0000").contains("U "), "{view}");
    assert!(view.contains("E1: reviewed path"), "{view}");
}
