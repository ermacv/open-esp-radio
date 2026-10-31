use super::*;

fn cause(code: u32) -> ResetCause {
    ResetCauseField {
        address: 0x2070_1030,
        bit_offset: 1,
        bit_width: 6,
        power_on: 1,
    }
    .cause(code << 1 | 1)
}

#[test]
fn a_power_cycle_needs_the_board_to_leave_and_return() {
    let after = Duration::from_millis(3100);
    let cycle = |left, returned| PowerCycle { left, returned };
    assert_eq!(cycle(true, Some(after)).verdict(), Ok(after));
    assert!(
        cycle(false, Some(after))
            .verdict()
            .unwrap_err()
            .contains("does not cut its power")
    );
    assert!(
        cycle(true, None)
            .verdict()
            .unwrap_err()
            .contains("did not return")
    );
}

#[test]
fn a_power_loss_also_needs_a_power_on_reset() {
    let after = Duration::from_millis(3100);
    let loss = |left, returned, reset| PowerLoss {
        cycle: PowerCycle { left, returned },
        reset,
    };
    assert_eq!(
        loss(true, Some(after), Some(Ok(cause(1)))).verdict(),
        Ok(after)
    );
    assert!(
        loss(false, Some(after), Some(Ok(cause(1))))
            .verdict()
            .is_err()
    );
    // Dropped from the bus and back without losing power: the board keeps
    // the cause of its last reset, a JTAG reset here.
    let kept_power = loss(true, Some(after), Some(Ok(cause(0x18))))
        .verdict()
        .unwrap_err();
    assert!(
        kept_power.contains("0x18, not a power-on reset"),
        "{kept_power}"
    );
    let unread = loss(true, Some(after), Some(Err(String::from("no JTAG"))))
        .verdict()
        .unwrap_err();
    assert!(unread.contains("could not be read: no JTAG"), "{unread}");
    assert!(loss(true, Some(after), None).verdict().is_err());
}
