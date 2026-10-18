use super::*;
use std::cell::RefCell;

/// A port whose chip restarts on RTS's rising edge and speaks at once, as
/// the chip-b and chip-a USB Serial/JTAG do.
struct Port {
    rts: bool,
    input: Vec<&'static str>,
}

fn reset(port: &RefCell<Port>) {
    sequence(
        |step| {
            let mut port = port.borrow_mut();
            match step {
                Step::Rts(level) => {
                    if level && !port.rts {
                        port.input.push("new boot");
                    }
                    port.rts = level;
                }
                Step::ClearInput => port.input.clear(),
                Step::Dtr(_) => {}
            }
            Ok::<_, ()>(())
        },
        || {
            let mut port = port.borrow_mut();
            if !port.rts {
                port.input.push("old event");
            }
        },
    )
    .unwrap();
}

#[test]
fn the_new_boot_keeps_what_it_sends_before_the_release() {
    let port = RefCell::new(Port {
        rts: false,
        input: vec!["previous event"],
    });
    reset(&port);
    assert_eq!(port.into_inner().input, ["new boot"]);
}

#[test]
fn failed_input_drain_does_not_reset_into_an_ambiguous_boot() {
    let mut asserted = false;
    let result = sequence(
        |step| match step {
            Step::ClearInput => Err("drain failed"),
            Step::Rts(true) => {
                asserted = true;
                Ok(())
            }
            _ => Ok(()),
        },
        || {},
    );
    assert_eq!(result, Err("drain failed"));
    assert!(!asserted);
}

#[test]
fn a_download_reset_holds_the_boot_strap_across_the_reset_edge_and_releases_both() {
    let steps = std::cell::RefCell::new(Vec::new());
    download_sequence(
        |step| {
            steps.borrow_mut().push(match step {
                Step::Dtr(level) => ("dtr", level),
                Step::Rts(level) => ("rts", level),
                Step::ClearInput => ("clear", true),
            });
            Ok::<_, ()>(())
        },
        || steps.borrow_mut().push(("settle", true)),
    )
    .unwrap();
    let steps = steps.into_inner();
    // The boot strap is asserted before RTS's rising edge.
    let strap = steps
        .iter()
        .position(|step| *step == ("dtr", true))
        .unwrap();
    let edge = steps
        .iter()
        .position(|step| *step == ("rts", true))
        .unwrap();
    assert!(strap < edge, "{steps:?}");
    assert!(
        steps[strap..edge].contains(&("settle", true)),
        "the strap settles first"
    );
    // Both lines end released, so the console is left without a reset.
    assert_eq!(steps[steps.len() - 2..], [("dtr", false), ("rts", false)]);
}
