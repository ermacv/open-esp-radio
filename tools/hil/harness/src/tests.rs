use std::{thread, time::Duration};

use oer_hil_link::test_support::{Device, HELLO};
use oer_hil_protocol::{
    Endpoint, Key, Message, WireKind,
    base::{GetImageKeys, Hello, ImageKeyPage, RejectReason, Rejected},
};
use serde::{Deserialize, Serialize};

use crate::{Case, Cx, Failure, Outcome, Scenario, Verdict, check, run};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Ask(u8);
impl Message for Ask {
    const PATH: &'static str = "test/ask";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Ask {
    type Response = Told;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Told(u8);
impl Message for Told {
    const PATH: &'static str = "test/told";
}

const SERVICE: &[Key] = &[Ask::KEY, Told::KEY];
const SCENARIO: Scenario = Scenario {
    name: "test",
    image: "test-image",
    services: &[SERVICE],
};

/// The identity of the image the tests built.
const IMAGE: u32 = 0x5eed_0001;

/// The Hello of the built image, which serves the test service.
fn hello() -> Hello {
    Hello {
        keys: SERVICE.len() as u16,
        image_crc32: IMAGE,
        ..HELLO
    }
}

/// Serve the image's key pages, then answer `Ask(n)` with `Told(n + 1)`,
/// refuse `Ask(0)`, leave `Ask(1)` unanswered and reboot on `Ask(2)`.
fn serve(mut device: Device) {
    device.send(0, hello());
    while let Some(request) = device.request() {
        if let Some(GetImageKeys { first }) = request.decode() {
            let keys = SERVICE
                .iter()
                .copied()
                .skip(usize::from(first))
                .take(1)
                .collect();
            device.send(
                request.request_id,
                ImageKeyPage {
                    first,
                    total: SERVICE.len() as u16,
                    keys,
                },
            );
        } else if let Some(Ask(value)) = request.decode() {
            match value {
                0 => device.send(request.request_id, Rejected(RejectReason::Busy)),
                1 => {}
                2 => device.reboot(device.boot_id + 1, hello()),
                _ => device.send(request.request_id, Told(value + 1)),
            }
        }
    }
}

fn ask(value: u8) -> impl Fn(&mut Cx<'_>) -> Result<(), Failure> {
    move |cx| {
        let Told(answer) = cx.call(Ask(value), Duration::from_millis(300))?;
        check(answer == value + 1, "the answer is one more")
    }
}

fn verdicts(outcomes: &[Outcome]) -> Vec<(Option<&'static str>, u8)> {
    outcomes
        .iter()
        .map(|outcome| (outcome.case, outcome.verdict.exit_code()))
        .collect()
}

#[test]
fn cases_pass_fail_and_a_reboot_fails_its_case_but_not_the_next() {
    let (device, link) = Device::new(10);
    let server = thread::spawn(move || serve(device));
    let cases = [
        Case {
            name: "answers",
            run: |cx| ask(7)(cx),
        },
        Case {
            name: "refused",
            run: |cx| ask(0)(cx),
        },
        Case {
            name: "reboots",
            run: |cx| ask(2)(cx),
        },
        Case {
            name: "after-reboot",
            run: |cx| {
                check(cx.boot().boot_id == 11, "the case runs on the new boot")?;
                ask(9)(cx)
            },
        },
    ];
    let selected = cases.iter().collect::<Vec<_>>();
    let outcomes = run::cases(&link, &SCENARIO, IMAGE, &selected);
    assert_eq!(
        verdicts(&outcomes),
        [
            (Some("answers"), 0),
            (Some("refused"), 1),
            (Some("reboots"), 1),
            (Some("after-reboot"), 0)
        ],
        "{outcomes:?}"
    );
    drop(link);
    server.join().unwrap();
}

#[test]
fn an_unconfirmed_request_leaves_the_rest_of_its_boot_incomplete() {
    let (device, link) = Device::new(20);
    let server = thread::spawn(move || serve(device));
    let cases = [
        Case {
            name: "silent",
            run: |cx| ask(1)(cx),
        },
        Case {
            name: "next",
            run: |cx| ask(7)(cx),
        },
    ];
    let selected = cases.iter().collect::<Vec<_>>();
    let outcomes = run::cases(&link, &SCENARIO, IMAGE, &selected);
    assert_eq!(
        verdicts(&outcomes),
        [(Some("silent"), 2), (Some("next"), 2)],
        "{outcomes:?}"
    );
    drop(link);
    server.join().unwrap();
}

#[test]
fn another_image_than_the_built_one_runs_no_case() {
    let (mut device, link) = Device::new(40);
    let server = thread::spawn(move || {
        device.send(
            0,
            Hello {
                image_crc32: IMAGE + 1,
                ..hello()
            },
        );
        while device.request().is_some() {}
    });
    let cases = [Case {
        name: "answers",
        run: |cx| ask(7)(cx),
    }];
    let selected = cases.iter().collect::<Vec<_>>();
    let outcomes = run::cases(&link, &SCENARIO, IMAGE, &selected);
    let [
        Outcome {
            case: None,
            verdict: Verdict::Incomplete(reason),
        },
    ] = outcomes.as_slice()
    else {
        panic!("{outcomes:?}");
    };
    assert!(
        reason.contains("5eed0002") && reason.contains("5eed0001"),
        "{reason}"
    );
    drop(link);
    server.join().unwrap();
}

#[test]
fn an_image_without_the_scenarios_service_runs_no_case() {
    let (mut device, link) = Device::new(30);
    let server = thread::spawn(move || {
        device.send(
            0,
            Hello {
                image_crc32: IMAGE,
                ..HELLO
            },
        );
        while device.request().is_some() {}
    });
    let cases = [Case {
        name: "answers",
        run: |cx| ask(7)(cx),
    }];
    let selected = cases.iter().collect::<Vec<_>>();
    let outcomes = run::cases(&link, &SCENARIO, IMAGE, &selected);
    assert_eq!(verdicts(&outcomes), [(None, 2)], "{outcomes:?}");
    let Verdict::Incomplete(reason) = &outcomes[0].verdict else {
        panic!("{outcomes:?}");
    };
    assert!(reason.contains(&Ask::KEY.to_string()), "{reason}");
    drop(link);
    server.join().unwrap();
}

#[test]
fn a_failure_outranks_an_incomplete_case_and_no_case_is_no_pass() {
    let outcome = |case, verdict| Outcome {
        case: Some(case),
        verdict,
    };
    let mut out = Vec::new();
    let verdict = run::report(
        &SCENARIO,
        &[
            outcome("a", Verdict::Incomplete("link".into())),
            outcome("b", Verdict::Fail("wrong".into())),
            outcome("c", Verdict::Pass),
        ],
        &mut out,
    );
    assert_eq!(verdict, Verdict::Fail("wrong".into()));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("case b: fail: wrong"), "{text}");
    assert!(
        text.ends_with("scenario test: 1 pass, 1 fail, 1 incomplete\n"),
        "{text}"
    );
    let incomplete = run::report(
        &SCENARIO,
        &[outcome("a", Verdict::Incomplete("x".into()))],
        &mut Vec::new(),
    );
    assert_eq!(incomplete.exit_code(), 2);
    assert_eq!(run::report(&SCENARIO, &[], &mut Vec::new()).exit_code(), 2);
}

#[test]
fn a_case_filter_keeps_the_scenarios_order_and_refuses_an_unknown_name() {
    let cases = [
        Case {
            name: "a",
            run: |_| Ok(()),
        },
        Case {
            name: "b",
            run: |_| Ok(()),
        },
    ];
    let names = |selected: Vec<&Case>| selected.iter().map(|case| case.name).collect::<Vec<_>>();
    assert_eq!(names(run::select(&cases, &[]).unwrap()), ["a", "b"]);
    assert_eq!(
        names(run::select(&cases, &["b".into(), "a".into()]).unwrap()),
        ["a", "b"]
    );
    assert!(run::select(&cases, &["c".into()]).is_err());
}
