use super::*;
use oer_diagnostics::{Console, PreviousPanic};
use oer_trace::{Channel, Kind};

fn write(bytes: &mut Vec<u8>) -> impl FnMut(&[u8]) -> Result<(), ()> + '_ {
    |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    }
}

fn rendered(bytes: &[u8]) -> oer_image_build::Result<String> {
    let mut output = Vec::new();
    render_stream(&mut std::io::Cursor::new(bytes), &mut output)?;
    Ok(String::from_utf8(output).unwrap())
}

#[test]
fn human_panic_text_preserves_static_formatted_empty_and_interrupt_fields() {
    let mut bytes = Vec::new();
    {
        let mut console = Console::new(write(&mut bytes));
        let mut panic = PreviousPanic {
            hart: 0,
            in_interrupt: false,
            interrupted_pc: None,
            file: "src/main.rs",
            line: 42,
            column: 7,
            message: Some("test panic"),
        };
        console.previous_panic(&panic).unwrap();
        panic.message = None;
        console.previous_panic(&panic).unwrap();
        panic.message = Some("");
        console.previous_panic(&panic).unwrap();
        panic.hart = 1;
        panic.in_interrupt = true;
        panic.interrupted_pc = Some(0x500123ab);
        console.previous_panic(&panic).unwrap();
    }
    let lines = rendered(&bytes)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "OER-PANIC previous src/main.rs:42:7 on hart 0: test panic",
            "OER-PANIC previous src/main.rs:42:7 on hart 0 (formatted message not kept)",
            "OER-PANIC previous src/main.rs:42:7 on hart 0: ",
            "OER-PANIC previous src/main.rs:42:7 on hart 1 in an interrupt of 0x500123ab: ",
        ]
    );
}

#[test]
fn radio_root_round_trip_renders_every_clock_and_failure_as_before() {
    use oer_espressif_esp_hal_radio::diagnostics::RadioRoot;
    use oer_espressif_esp_hal_radio::diagnostics::RadioRoot::*;
    use oer_trace::Event;
    // The same explicit wire ids must decode as the original semantic event.
    let mut events = vec![Handoff, RootTaken];
    for clock in 0..5 {
        events.push(RadioRoot::decode([2, clock]).unwrap());
        events.push(RadioRoot::decode([3, clock]).unwrap());
    }
    events.extend([Passed, ClockFailed]);
    let mut bytes = Vec::new();
    {
        let mut console = Console::new(write(&mut bytes));
        for event in &events {
            assert_eq!(RadioRoot::decode(event.encode()), Some(*event));
            console.event(event).unwrap();
        }
    }
    let text = rendered(&bytes).unwrap();
    let lines = text
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines[0], "OER radio-root handoff=ok");
    assert_eq!(lines[1], "OER-RESULT: fail radio-root take_radio=RootTaken");
    for (i, clock) in [
        "Pll160m",
        "AnalogI2cMaster",
        "Mpll",
        "ModemCoexistence",
        "ModemLowPowerTimer",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            lines[2 + i * 2],
            format!("OER radio-root clock={clock} acquired released")
        );
        assert_eq!(
            lines[3 + i * 2],
            format!("OER radio-root clock={clock} refused=PlatformClockError")
        );
    }
    assert_eq!(lines[12], "OER-RESULT: pass radio-root");
    assert_eq!(lines[13], "OER-RESULT: fail radio-root clock");
    assert_eq!(RadioRoot::decode([99, 0]), None);
    assert_eq!(RadioRoot::decode([2, 5]), None);
    assert_eq!(RadioRoot::decode([0, 1]), None);
}

#[test]
fn boot_verdict_cannot_hide_lost_panic_or_partial_tail() {
    let mut bytes = Vec::new();
    {
        let mut console = Console::new(write(&mut bytes));
        console
            .previous_panic(&PreviousPanic {
                hart: 0,
                in_interrupt: false,
                interrupted_pc: None,
                file: "panic.rs",
                line: 1,
                column: 2,
                message: None,
            })
            .unwrap();
        console.event(&BootSmokePassed).unwrap();
    }
    assert!(
        rendered(&bytes)
            .unwrap()
            .contains("OER-RESULT: pass boot-smoke")
    );
    let last = bytes
        .windows(oer_diagnostics::PREFIX.len())
        .rposition(|part| part == oer_diagnostics::PREFIX)
        .unwrap();
    assert!(
        rendered(&bytes[last..])
            .unwrap_err()
            .to_string()
            .contains("loss/order")
    );
    for end in last + oer_diagnostics::PREFIX.len()..bytes.len() - 1 {
        assert!(rendered(&bytes[..end]).is_err(), "cut {end}");
    }
}

#[test]
fn unknown_event_is_a_handled_error_and_rom_reset_restarts_sequence() {
    struct Unknown;
    impl Event for Unknown {
        const KIND: Kind = Kind::new(oer_trace::Domain::Platform, 255);
        const CHANNEL: Channel = BootSmokePassed::CHANNEL;
        fn encode(&self) -> [u32; 2] {
            [0, 0]
        }
        fn decode(_: [u32; 2]) -> Option<Self> {
            None
        }
    }
    let mut bytes = Vec::new();
    Console::new(write(&mut bytes)).event(&Unknown).unwrap();
    assert!(
        rendered(&bytes)
            .unwrap_err()
            .to_string()
            .contains("unknown boot diagnostic")
    );
    bytes.clear();
    Console::new(write(&mut bytes))
        .event(&BootSmokePassed)
        .unwrap();
    let mut repeated = bytes.clone();
    repeated.extend_from_slice(&bytes);
    assert!(rendered(&repeated).is_err());
    let mut reset = bytes.clone();
    reset.extend_from_slice(b"rst:0x0 boot:0x0\r\n");
    reset.extend_from_slice(&bytes);
    assert_eq!(
        rendered(&reset)
            .unwrap()
            .matches("OER-RESULT: pass boot-smoke")
            .count(),
        2
    );
}

#[test]
fn arbitrary_read_boundaries_preserve_the_rendered_records() {
    let mut bytes = b"ROM banner\r\n".to_vec();
    Console::new(write(&mut bytes))
        .event(&BootSmokePassed)
        .unwrap();
    for capacity in [1, 2, 7, 64] {
        let mut reader = std::io::BufReader::with_capacity(capacity, std::io::Cursor::new(&bytes));
        let mut output = Vec::new();
        render_stream(&mut reader, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "ROM banner\n\nOER-RESULT: pass boot-smoke\n"
        );
    }
}
