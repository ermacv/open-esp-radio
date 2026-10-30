//! A synthetic scenario on stand doubles: equipment of one type in two
//! slots, an event before its request's response, a lost required event and
//! a reset, all through the one life cycle the harness runs.

use std::{cell::Cell, rc::Rc, time::Duration};

use oer_hil_link::test_support::Device;
use oer_hil_protocol::{Endpoint, Key, Message, WireKind, base::ImageKeyPage};
use oer_lab_model::Stand;
use serde::{Deserialize, Serialize};

use super::{Equipment, Opened, Resolved, Slot, resolve};
use crate::fakes::{FakeBoard, IMAGE, hello, rts};
use crate::{Case, Cx, Failure, Outcome, Scenario, Verdict, check, dut::Dut, run};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Ask(u8);
impl Message for Ask {
    const PATH: &'static str = "synthetic/ask";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Ask {
    type Response = Told;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Told(u8);
impl Message for Told {
    const PATH: &'static str = "synthetic/told";
}

/// An event of a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Progress(u8);
impl Message for Progress {
    const PATH: &'static str = "synthetic/progress";
}

/// The event that ends a request's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
struct Done(u8);
impl Message for Done {
    const PATH: &'static str = "synthetic/done";
}

/// A lamp: equipment whose glow is a long operation with its own handle.
struct Lamp {
    resource: String,
    abandoned: Rc<Cell<bool>>,
}

/// A glow of a lamp: `finish` or `cancel` ends it; dropped without either,
/// it is abandoned and the lamp's finish stops and reports it.
struct Glow {
    ended: bool,
    abandoned: Rc<Cell<bool>>,
}

impl Glow {
    fn finish(mut self) -> u32 {
        self.ended = true;
        42
    }
}

impl Drop for Glow {
    fn drop(&mut self) {
        if !self.ended {
            self.abandoned.set(true);
        }
    }
}

impl Lamp {
    fn glow(&mut self) -> Glow {
        Glow {
            ended: false,
            abandoned: Rc::clone(&self.abandoned),
        }
    }
}

impl Equipment for Lamp {
    fn resolve(_stand: &Stand, slot: &'static str) -> Result<Resolved, String> {
        Ok(Resolved {
            resource: format!("lamp at {slot}"),
            claims: Vec::new(),
        })
    }
    fn open(resolved: &Resolved) -> Result<Self, String> {
        Ok(Self {
            resource: resolved.resource.clone(),
            abandoned: Rc::new(Cell::new(false)),
        })
    }
    fn finish(&mut self) -> Result<(), String> {
        if self.abandoned.get() {
            return Err("a glow was abandoned; it was stopped".to_owned());
        }
        Ok(())
    }
}

/// Equipment the stand file has only one of.
struct Only;

impl Equipment for Only {
    fn resolve(_stand: &Stand, _slot: &'static str) -> Result<Resolved, String> {
        Ok(Resolved {
            resource: "the only one".to_owned(),
            claims: Vec::new(),
        })
    }
    fn open(_resolved: &Resolved) -> Result<Self, String> {
        Ok(Self)
    }
    fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }
}

const LAMP_A: Slot<Lamp> = Slot::new("lamp-a");
const LAMP_B: Slot<Lamp> = Slot::new("lamp-b");
const SERVICE: &[Key] = &[Ask::KEY, Told::KEY, Progress::KEY, Done::KEY];
const SERVED: [Key; 4] =
    oer_hil_protocol::base::sorted_keys([Ask::KEY, Told::KEY, Progress::KEY, Done::KEY]);
const SCENARIO: Scenario = crate::scenario!(
    name: "synthetic",
    image: "synthetic-image",
    services: &[SERVICE],
    equipment: &[LAMP_A.declare(), LAMP_B.declare()],
);

fn stand() -> Stand {
    toml::from_str("schema = 1").unwrap()
}

/// The synthetic device: its key pages, then `Ask(1)` answered after a
/// related `Progress`, `Ask(2)` answered with a `Progress` after a gap in its
/// sequence and never a `Done`, and any other `Ask(n)` with `Told(n + 1)`.
fn serve(mut device: Device) {
    while let Some(request) = device.request() {
        if let Some(oer_hil_protocol::base::GetImageKeys { first }) = request.decode() {
            device.send(
                request.request_id,
                ImageKeyPage {
                    first,
                    total: SERVED.len() as u16,
                    keys: SERVED.iter().copied().skip(usize::from(first)).collect(),
                },
            );
        } else if let Some(Ask(value)) = request.decode() {
            match value {
                1 => {
                    device.send(request.request_id, Progress(1));
                    device.send(request.request_id, Told(2));
                }
                2 => {
                    device.send(request.request_id, Told(3));
                    device.sequence += 3;
                    device.send(request.request_id, Progress(2));
                }
                _ => device.send(request.request_id, Told(value + 1)),
            }
        } else if request
            .decode::<oer_hil_protocol::base::GetHello>()
            .is_some()
        {
            device.send(request.request_id, synthetic_hello());
        }
    }
}

fn synthetic_hello() -> oer_hil_protocol::base::Hello {
    oer_hil_protocol::base::Hello {
        keys: SERVED.len() as u16,
        keys_digest: oer_hil_protocol::base::digest(&SERVED),
        ..hello()
    }
}

const WAIT: Duration = Duration::from_millis(500);

fn early_event(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let cursor = cx.events()?;
    let Told(answer) = cx.call(Ask(1), WAIT)?;
    let Progress(step) = cx.event(&cursor, WAIT)?;
    check(
        (answer, step) == (2, 1),
        "the response and its earlier event",
    )
}

fn two_lamps(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let a = cx.take(&LAMP_A)?;
    let mut b = cx.take(&LAMP_B)?;
    check(a.resource != b.resource, "two slots are two lamps")?;
    check(
        matches!(cx.take(&LAMP_A), Err(Failure::Incomplete(reason)) if reason.contains("taken")),
        "a held slot is not taken twice",
    )?;
    drop(a);
    drop(cx.take(&LAMP_A)?);
    check(
        b.glow().finish() == 42,
        "a finished glow returns its result",
    )?;
    // Dropped without finish: abandoned, for the session to stop.
    let _abandoned = b.glow();
    Ok(())
}

fn lost_event(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let cursor = cx.events()?;
    cx.call(Ask(2), WAIT)?;
    cx.event::<Done>(&cursor, Duration::from_millis(200))?;
    Ok(())
}

fn reset(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let before = cx.boot().boot_id;
    let after = cx.reset()?.boot_id;
    check(after != before, "the reset started a new boot")?;
    let Told(answer) = cx.call(Ask(5), WAIT)?;
    check(answer == 6, "the new boot answers")
}

#[test]
fn a_synthetic_scenario_runs_through_the_one_life_cycle() {
    let mut board = FakeBoard::new(vec![rts(true, true)]);
    board.serve = |mut device| {
        device.send(0, synthetic_hello());
        serve(device);
    };
    board.image_up.set(true);
    let link = {
        let (mut device, link) = Device::new(1);
        device.send(0, synthetic_hello());
        std::thread::spawn(move || serve(device));
        link
    };
    let slots = resolve(&stand(), SCENARIO.equipment).unwrap();
    let mut equipment = Opened::default();
    equipment.open(&slots).unwrap();
    let cases = [
        Case {
            name: "early-event",
            run: early_event,
        },
        Case {
            name: "two-lamps",
            run: two_lamps,
        },
        Case {
            name: "lost-event",
            run: lost_event,
        },
        Case {
            name: "reset",
            run: reset,
        },
    ];
    let selected = cases.iter().collect::<Vec<_>>();
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(WAIT);
    let mut out = Vec::new();
    let run = run::cases(&mut dut, &equipment, &SCENARIO, &selected, &mut out);
    let verdicts = run
        .outcomes
        .iter()
        .map(|Outcome { case, verdict }| (*case, verdict.exit_code()))
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts,
        [
            (Some("early-event"), 0),
            (Some("two-lamps"), 0),
            (Some("lost-event"), 2),
            (Some("reset"), 0)
        ],
        "{:?}",
        run.outcomes
    );
    let Verdict::Incomplete(reason) = &run.outcomes[2].verdict else {
        panic!("{:?}", run.outcomes[2]);
    };
    assert!(reason.contains("lost"), "{reason}");
    // Every outcome was printed as it came.
    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.lines().count(), 4, "{printed}");
    assert_eq!(board.resets.get(), 1);
    assert_eq!(
        equipment.finish(),
        [("lamp-b", "a glow was abandoned; it was stopped".to_owned())]
    );
}

#[test]
fn two_slots_on_one_resource_are_refused_before_the_lease() {
    const FIRST: Slot<Only> = Slot::new("first");
    const SECOND: Slot<Only> = Slot::new("second");
    let refused = resolve(&stand(), &[FIRST.declare(), SECOND.declare()]).unwrap_err();
    assert!(refused.contains("`first` and `second`"), "{refused}");
}

#[test]
fn two_slots_of_one_name_are_not_distinct() {
    const DUPLICATE: &[super::Declared] = &[LAMP_A.declare(), LAMP_A.declare()];
    let scenario = Scenario::unchecked("duplicate", "image", &[], DUPLICATE);
    assert!(!scenario.slots_distinct());
    assert!(SCENARIO.slots_distinct());
}
