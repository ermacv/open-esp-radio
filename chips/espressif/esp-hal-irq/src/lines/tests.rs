use super::*;

fn line(level: u8) -> LineState {
    LineState {
        enabled: true,
        vectored: true,
        trigger: LEVEL_TRIGGERED,
        control: control_of(level),
    }
}

#[test]
fn each_level_has_its_line_and_control_byte() {
    assert_eq!((line_of(1), control_of(1)), (16, 0x1f));
    assert_eq!((line_of(2), control_of(2)), (17, 0x3f));
    assert_eq!((line_of(3), control_of(3)), (18, 0x5f));
    assert_eq!((line_of(8), control_of(8)), (23, 0xff));
    for level in 1..=LEVELS {
        assert_eq!(level_of_control(control_of(level)), Some(level));
        assert_eq!(check_line(level, line(level)), Ok(()));
    }
    assert_eq!(level_of_control(0x00), None);
}

#[test]
fn a_control_byte_of_another_level_is_refused() {
    assert_eq!(
        check_line(2, line(3)),
        Err(LineError::WrongControl {
            line: 17,
            control: 0x5f,
            expected: 0x3f
        })
    );
}

#[test]
fn a_disabled_unvectored_or_edge_line_is_refused() {
    let disabled = LineState {
        enabled: false,
        ..line(1)
    };
    assert_eq!(
        check_line(1, disabled),
        Err(LineError::Disabled { line: 16 })
    );
    let unvectored = LineState {
        vectored: false,
        ..line(1)
    };
    assert_eq!(
        check_line(1, unvectored),
        Err(LineError::NotVectored { line: 16 })
    );
    let edge = LineState {
        trigger: 1,
        ..line(3)
    };
    assert_eq!(
        check_line(3, edge),
        Err(LineError::NotLevelTriggered {
            line: 18,
            trigger: 1
        })
    );
}

#[test]
fn a_source_is_silent_or_on_its_level_line() {
    assert_eq!(check_route(2, 0), Ok(()));
    assert_eq!(check_route(2, 17), Ok(()));
    assert_eq!(
        check_route(2, 18),
        Err(LineError::WrongLine {
            line: 18,
            expected: 17
        })
    );
    assert_eq!(check_route(9, 0), Err(LineError::NoSuchLevel { level: 9 }));
    assert_eq!(
        check_line(0, line(1)),
        Err(LineError::NoSuchLevel { level: 0 })
    );
}
