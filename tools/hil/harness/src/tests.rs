use std::{thread, time::Duration};

use oer_hil_link::test_support::{Device, HELLO};
use oer_hil_protocol::{
    Endpoint, Key, Message, WireKind,
    base::{GetImageKeys, Hello, ImageKeyPage, RejectReason, Rejected, digest, sorted_keys},
};
use serde::{Deserialize, Serialize};

use crate::{Case, Cleanup, Cx, Failure, Outcome, Scenario, Teardown, Verdict, check, run};

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
const SCENARIO: Scenario = crate::scenario!(
    name: "test",
    image: "test-image",
    services: &[SERVICE],
    equipment: &[],
);

/// Run `cases` on the device behind `link`, with no equipment.
fn run_cases(link: oer_hil_link::Link, cases: &[&Case]) -> Vec<Outcome> {
    let board = crate::fakes::FakeBoard::new(Vec::new());
    let mut dut = crate::dut::Dut::new(Some(link), &board, IMAGE);
    run::cases(
        &mut dut,
        &crate::equipment::Opened::default(),
        &SCENARIO,
        cases,
        &mut Vec::new(),
    )
    .outcomes
}

/// The identity of the image the tests built.
const IMAGE: u32 = 0x5eed_0001;

/// The image's capability set: the service's keys, ascending.
const SERVED: [Key; 2] = sorted_keys([Ask::KEY, Told::KEY]);

/// The Hello of the built image, which serves the test service.
fn hello() -> Hello {
    Hello {
        keys: SERVED.len() as u16,
        keys_digest: digest(&SERVED),
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
            let keys = SERVED
                .iter()
                .copied()
                .skip(usize::from(first))
                .take(1)
                .collect();
            device.send(
                request.request_id,
                ImageKeyPage {
                    first,
                    total: SERVED.len() as u16,
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
    let outcomes = run_cases(link, &selected);
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
    let outcomes = run_cases(link, &selected);
    assert_eq!(
        verdicts(&outcomes),
        [(Some("silent"), 2), (Some("next"), 2)],
        "{outcomes:?}"
    );
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
    let outcomes = run_cases(link, &selected);
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
    server.join().unwrap();
}

#[test]
fn an_image_without_the_scenarios_service_runs_no_case() {
    let (mut device, link) = Device::new(30);
    let server = thread::spawn(move || {
        device.send(
            0,
            Hello {
                keys_digest: digest(&[]),
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
    let outcomes = run_cases(link, &selected);
    assert_eq!(verdicts(&outcomes), [(None, 2)], "{outcomes:?}");
    let Verdict::Incomplete(reason) = &outcomes[0].verdict else {
        panic!("{outcomes:?}");
    };
    assert!(reason.contains(&Ask::KEY.to_string()), "{reason}");
    server.join().unwrap();
}

#[test]
fn key_pages_other_than_the_announced_set_run_no_case() {
    let page = |first: u16, keys: &[Key]| ImageKeyPage {
        first,
        total: 2,
        keys: keys.iter().copied().collect(),
    };
    let [low, high] = SERVED;
    let wrong_digest = Hello {
        keys_digest: digest(&SERVED) ^ 1,
        ..hello()
    };
    let three = Hello { keys: 3, ..hello() };
    let three_page = |first: u16, keys: &[Key]| ImageKeyPage {
        total: 3,
        ..page(first, keys)
    };
    for (what, hello, pages) in [
        ("wrong digest", wrong_digest, vec![page(0, &SERVED)]),
        (
            "repeated key",
            hello(),
            vec![page(0, &[low]), page(1, &[low])],
        ),
        ("permuted keys", hello(), vec![page(0, &[high, low])]),
        (
            "last page too long",
            hello(),
            vec![page(0, &[low]), page(1, &[high, Key([0xff; 8])])],
        ),
        (
            "empty page before the end",
            three,
            vec![
                three_page(0, &[low]),
                three_page(1, &[]),
                three_page(1, &[high]),
            ],
        ),
    ] {
        let (mut device, link) = Device::new(50);
        let server = thread::spawn(move || {
            device.send(0, hello);
            let mut pages = pages.into_iter();
            let mut asked = false;
            while let Some(request) = device.request() {
                if request.decode::<GetImageKeys>().is_some() {
                    if let Some(page) = pages.next() {
                        device.send(request.request_id, page);
                    }
                } else {
                    asked = true;
                }
            }
            asked
        });
        let cases = [Case {
            name: "answers",
            run: |cx| ask(7)(cx),
        }];
        let selected = cases.iter().collect::<Vec<_>>();
        let outcomes = run_cases(link, &selected);
        assert_eq!(verdicts(&outcomes), [(None, 2)], "{what}: {outcomes:?}");
        assert!(!server.join().unwrap(), "{what}: a case ran");
    }
}

#[test]
fn a_failure_outranks_an_incomplete_case_and_no_case_is_no_pass() {
    let outcome = |case, verdict| Outcome {
        case: Some(case),
        verdict,
    };
    let mut out = Vec::new();
    let verdict = run::summary(
        &SCENARIO,
        &[
            outcome("a", Verdict::Incomplete("link".into())),
            outcome("b", Verdict::Fail("wrong".into())),
            outcome("c", Verdict::Pass),
        ],
        Some(&Teardown {
            dut: Cleanup::Clean,
            equipment: Vec::new(),
        }),
        &mut out,
    );
    assert_eq!(verdict, Verdict::Fail("wrong".into()));
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text,
        "cleanup: clean\nscenario test: 1 pass, 1 fail, 1 incomplete\n"
    );
    let incomplete = run::summary(
        &SCENARIO,
        &[outcome("a", Verdict::Incomplete("x".into()))],
        None,
        &mut Vec::new(),
    );
    assert_eq!(incomplete.exit_code(), 2);
    assert_eq!(
        run::summary(&SCENARIO, &[], None, &mut Vec::new()).exit_code(),
        2
    );
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
