//! Filter accept list commands and the scanner that filters by the list.

use bt_hci::cmd::{Opcode, OpcodeGroup};
use oer_bluetooth_radio::{AcceptListChange, AcceptListDevice, RequestError, ScanFilterPolicy};

use super::{DISALLOWED, HARDWARE_FAILURE, Harness, RESET, Request, SET_SCAN_ENABLE, SUCCESS};

const CLEAR: Opcode = Opcode::new(OpcodeGroup::LE, 0x0010);
const ADD: Opcode = Opcode::new(OpcodeGroup::LE, 0x0011);
const REMOVE: Opcode = Opcode::new(OpcodeGroup::LE, 0x0012);
const MEMORY_CAPACITY_EXCEEDED: u8 = 0x07;

const RANDOM_DEVICE: [u8; 7] = [0x01, 0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1];

fn random_device() -> AcceptListDevice {
    AcceptListDevice {
        random: true,
        address: [0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1],
    }
}

impl<const C: usize, const O: usize> super::Bench<C, O> {
    /// Send one list command, answer its radio request with `result` and
    /// return the request and the command's status.
    fn change_list(
        &mut self,
        opcode: Opcode,
        parameters: &[u8],
        result: Result<(), RequestError>,
    ) -> Result<(Option<Request>, Option<u8>), crate::EpochExhausted> {
        assert_eq!(self.command(opcode, parameters), None);
        assert!(!self.core.is_command_ready());
        let request = self.step_with(result)?;
        Ok((request, self.status_of(opcode)))
    }
}

#[test]
fn list_commands_reach_the_backend_and_complete_with_its_answer() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness.change_list(ADD, &RANDOM_DEVICE, Ok(())).unwrap(),
        (
            Some(Request::FilterAcceptList(AcceptListChange::Add(
                random_device()
            ))),
            Some(SUCCESS)
        )
    );
    assert_eq!(
        harness.change_list(REMOVE, &RANDOM_DEVICE, Ok(())).unwrap(),
        (
            Some(Request::FilterAcceptList(AcceptListChange::Remove(
                random_device()
            ))),
            Some(SUCCESS)
        )
    );
    assert_eq!(
        harness.change_list(CLEAR, &[], Ok(())).unwrap(),
        (
            Some(Request::FilterAcceptList(AcceptListChange::Clear)),
            Some(SUCCESS)
        )
    );
}

#[test]
fn a_full_list_answers_memory_capacity_exceeded_and_other_refusals_hardware_failure() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness
            .change_list(ADD, &RANDOM_DEVICE, Err(RequestError::ListFull))
            .unwrap()
            .1,
        Some(MEMORY_CAPACITY_EXCEEDED)
    );
    assert_eq!(
        harness
            .change_list(ADD, &RANDOM_DEVICE, Err(RequestError::Busy))
            .unwrap()
            .1,
        Some(HARDWARE_FAILURE)
    );
    // Removing an absent device is an invalid parameter.
    assert_eq!(
        harness
            .change_list(REMOVE, &RANDOM_DEVICE, Err(RequestError::NotListed))
            .unwrap()
            .1,
        Some(0x12)
    );
}

#[test]
fn list_commands_wait_for_reset_and_malformed_ones_never_reach_the_backend() {
    let mut harness = Harness::new();
    assert_eq!(harness.command(ADD, &RANDOM_DEVICE), Some(DISALLOWED));

    let mut harness = Harness::configured();
    // Anonymous advertising needs no device-table entry.
    assert_eq!(
        harness.command(ADD, &[0xff, 0, 0, 0, 0, 0, 0]),
        Some(SUCCESS)
    );
    assert_eq!(
        harness.command(REMOVE, &[0xff, 0, 0, 0, 0, 0, 0]),
        Some(SUCCESS)
    );
    assert_eq!(harness.command(ADD, &[2, 1, 2, 3, 4, 5, 6]), Some(0x12));
    assert_eq!(harness.command(ADD, &RANDOM_DEVICE[..6]), Some(0x12));
    assert!(!harness.core.wants_radio());
}

#[test]
fn a_scanner_filtering_by_the_list_keeps_it_fixed_until_disabled() {
    let mut harness = Harness::configured();
    harness.scan_with(true, ScanFilterPolicy::AcceptListOnly, 0x20, 0x10, false);
    assert_eq!(harness.command(ADD, &RANDOM_DEVICE), Some(DISALLOWED));
    assert_eq!(harness.command(CLEAR, &[]), Some(DISALLOWED));

    assert_eq!(harness.command(SET_SCAN_ENABLE, &[0, 0]), None);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveScanner));
    assert_eq!(harness.status_of(SET_SCAN_ENABLE), Some(SUCCESS));
    assert_eq!(
        harness.change_list(CLEAR, &[], Ok(())).unwrap().1,
        Some(SUCCESS)
    );

    // A scanner that accepts every advertiser leaves the list free.
    harness.scan_with(false, ScanFilterPolicy::AcceptAll, 0x20, 0x10, false);
    assert_eq!(
        harness.change_list(ADD, &RANDOM_DEVICE, Ok(())).unwrap().1,
        Some(SUCCESS)
    );
}

#[test]
fn reset_clears_a_list_that_may_hold_a_device_before_it_completes() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness.change_list(ADD, &RANDOM_DEVICE, Ok(())).unwrap().1,
        Some(SUCCESS)
    );
    assert_eq!(harness.command(RESET, &[]), None);
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::FilterAcceptList(AcceptListChange::Clear))
    );
    assert_eq!(harness.status_of(RESET), Some(SUCCESS));

    // The cleared list needs no second clear.
    assert_eq!(harness.command(RESET, &[]), Some(SUCCESS));
}
