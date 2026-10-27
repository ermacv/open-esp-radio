use super::*;

fn branch(delta: i32, condition: u32) -> u32 {
    let n = delta as u32;
    ((n >> 12 & 1) << 31)
        | ((n >> 5 & 63) << 25)
        | (condition << 12)
        | ((n >> 1 & 15) << 8)
        | ((n >> 11 & 1) << 7)
        | 0x63
}

#[test]
fn an_external_conditional_branch_cannot_hide_the_destinations_frame() {
    // All six RV32 branch conditions use the same accounting as jal x0.
    let transfers = [0, 1, 4, 5, 6, 7]
        .map(|condition| branch(8, condition))
        .into_iter()
        .chain([call(TEXT + 4, TEXT + 12) & !(31 << 7)]);
    for transfer in transfers {
        let image = executable(&[
            Symbol {
                name: "root",
                words: vec![SP_DOWN_16, transfer, RET],
                frame: None,
            },
            leaf(None),
        ]);
        let analysis = analyze(&image, &[], &[]).unwrap();
        let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
        assert_eq!(bound.bytes, Some(48), "{transfer:#x}: must not report 16");
        assert_eq!(bound.path, [(TEXT, 16), (TEXT + 12, 32)]);
        assert!(bound.unresolved.is_empty());
        assert_eq!(
            analysis.functions[&TEXT].transfers[0].kind,
            TransferKind::Tail
        );
    }
}

#[test]
fn an_external_conditional_branch_can_reach_an_earlier_function() {
    let image = executable(&[
        leaf(None),
        Symbol {
            name: "root",
            words: vec![SP_DOWN_16, branch(-16, 0), RET],
            frame: None,
        },
    ]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    let bound = analysis
        .bound(TEXT + 12, &mut ledger(), &mut || Ok(()))
        .unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert_eq!(bound.path, [(TEXT + 12, 16), (TEXT, 32)]);
}

#[test]
fn an_external_conditional_branch_does_not_hide_an_invalid_or_unframed_target() {
    for (delta, destination, reason) in [
        (12, leaf(None), Reason::IntoFunction),
        (0x100, leaf(None), Reason::OutsideImage),
        (
            8,
            Symbol {
                name: "dynamic",
                words: vec![0x0005_0113 /* mv sp, a0 */, RET],
                frame: None,
            },
            Reason::Unframed,
        ),
    ] {
        let image = executable(&[
            Symbol {
                name: "root",
                words: vec![SP_DOWN_16, branch(delta, 0), RET],
                frame: None,
            },
            destination,
        ]);
        let analysis = analyze(&image, &[], &[]).unwrap();
        let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
        assert_eq!(bound.bytes, None, "{reason:?}");
        assert!(bound.reasons().contains_key(&reason), "{reason:?}");
    }
}
