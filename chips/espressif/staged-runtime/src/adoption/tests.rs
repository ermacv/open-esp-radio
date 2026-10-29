#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
#[path = "../../../boot/tests/support/adoption.rs"]
mod support;
use oer_espressif_boot::adoption::InitError;
use oer_espressif_interrupt_table::Router;
use oer_time::Duration;
use support::{Fake, Hardware, Log, Op};

oer_espressif_interrupt_table::__fake_controller!();

struct Chip<'a>(core::marker::PhantomData<&'a ()>);
struct Platform;
impl oer_espressif_platform_port::EarlyText for Platform {
    fn text(_: &[u8]) -> bool {
        true
    }
}
impl<'a> Binding for Chip<'a> {
    type Platform = Platform;
    type Driver = Fake<'a>;
    type Hardware = Hardware<'a>;
    type Controller = FakeController;
    fn split(hardware: Hardware<'a>) -> (Hardware<'a>, FakeController) {
        (hardware, FakeController::new(0))
    }
}
fn context(controller: FakeController) -> Adopted<FakeController> {
    Adopted {
        router: Router::install(controller, &[]).unwrap(),
        #[cfg(chip_vectors)]
        admission: crate::PreAdmission::new(),
    }
}

#[test]
fn runtime_is_published_after_driver_arming_image_preparation_and_mapping() {
    let log = Log::new(None);
    let ready = complete::<Chip<'_>, _, _>(
        log.owners(),
        |hardware| {
            log.push(Op::BeforeMapping);
            hardware
        },
        |controller| {
            log.push(Op::Mapping);
            Ok((23_u32, context(controller)))
        },
    )
    .unwrap();
    let (mut hardware, mapping, mut adopted) = ready.into_parts();
    assert_eq!(mapping, 23);
    assert_eq!(hardware.clock.id, 12);
    assert_eq!(hardware.resources.id, 15);
    assert!(core::ptr::eq(hardware.clock.log, &log));
    adopted.router().verify().unwrap();
    let events = log.events.borrow();
    let armed = events
        .iter()
        .find(|(_, op)| *op == Op::Running(2, true, false))
        .unwrap()
        .0;
    let before = events
        .iter()
        .find(|(_, op)| *op == Op::BeforeMapping)
        .unwrap()
        .0;
    let mapping = events.iter().find(|(_, op)| *op == Op::Mapping).unwrap().0;
    assert_eq!(
        before.checked_duration_since(armed),
        Some(Duration::from_micros(3))
    );
    assert_eq!(
        mapping.checked_duration_since(before),
        Some(Duration::from_micros(1))
    );
    drop(events);
    log.events.borrow_mut().clear();
    hardware.watchdogs.rtc.feed();
    assert_eq!(
        log.operations(),
        [
            Op::Key(2, oer_espressif_watchdog::WRITE_KEY),
            Op::Feed,
            Op::Key(2, oer_espressif_watchdog::LOCK)
        ]
    );
}

#[test]
fn initialization_failure_never_reaches_mapping_or_the_image_callback() {
    for error in [InitError::ConsoleInstalled, InitError::CoreControlInstalled] {
        let log = Log::new(Some(error));
        let result = complete::<Chip<'_>, _, ()>(
            log.owners(),
            |_| panic!("no image callback before successful initialization"),
            |_| panic!("no adoption on initialization failure"),
        );
        assert!(matches!(result, Err(Error::Initialization(actual)) if actual == error));
        assert!(!log.operations().contains(&Op::Timer0Clock));
    }
}

#[test]
fn context_failure_returns_the_original_observation_without_replaying_initialization() {
    let log = Log::new(None);
    let failure = BootError::WrongHart {
        actual: 1,
        expected: 0,
    };
    let result = complete::<Chip<'_>, _, ()>(log.owners(), core::convert::identity, |_| {
        log.push(Op::Mapping);
        Err(failure)
    });
    assert!(matches!(result, Err(Error::Context(actual)) if actual == failure));
    assert_eq!(
        log.operations().iter().filter(|op| **op == Op::Cpu).count(),
        1
    );
    assert_eq!(log.operations().last(), Some(&Op::Mapping));
    assert_eq!(Error::Context(failure).name().as_str(), "interrupt-hart");
    assert_eq!(
        Error::Initialization(InitError::ConsoleInstalled)
            .name()
            .as_str(),
        "console-installed"
    );
    assert_eq!(
        Error::Initialization(InitError::CoreControlInstalled)
            .name()
            .as_str(),
        "core-control-installed"
    );
}

#[test]
fn early_reset_and_retained_output_survives_context_admission_failure() {
    use core::cell::RefCell;
    use oer_diagnostics::fault::{Class, Domain, Record};

    let log = Log::new(None);
    let output = RefCell::new(Vec::new());
    let mut previous = Record::encode(
        Class::ConfigurationBoot,
        Domain::Boot,
        1,
        0,
        0,
        b"previous boot failure",
    )
    .unwrap();
    let result = complete::<Chip<'_>, _, ()>(
        log.owners(),
        |hardware| {
            assert!(log.operations().contains(&Op::Console));
            crate::diagnostics::reset_cause(0, hardware.resources.id.into(), |part| {
                output.borrow_mut().extend_from_slice(part);
                Ok::<_, ()>(())
            })
            .unwrap();
            previous
                .report(|view| {
                    output.borrow_mut().extend_from_slice(view.payload);
                    Ok::<_, ()>(())
                })
                .unwrap();
            hardware.watchdogs
        },
        |_| {
            assert_eq!(
                output.borrow().as_slice(),
                b"reset cause hart=00000000 raw=0000000f retained=best-effort\nprevious boot failure"
            );
            Err(BootError::WrongHart {
                actual: 1,
                expected: 0,
            })
        },
    );
    assert!(matches!(
        result,
        Err(Error::Context(BootError::WrongHart { .. }))
    ));
    assert!(previous.peek().is_err());
    assert!(!output.borrow().is_empty());
}
