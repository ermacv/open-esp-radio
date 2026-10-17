use super::*;

#[test]
fn a_transfer_ends_when_both_directions_reach_their_end() {
    let mut status = AxiGdmaMem2MemStatus::default();
    status.rx.success_eof = true;
    assert!(!status.terminal());
    status.tx.total_eof = true;
    assert!(status.terminal() && !status.failed());
}

#[test]
fn either_direction_fails_the_transfer_before_its_end() {
    let rx_failures = [
        AxiGdmaMem2MemRxStatus {
            error_eof: true,
            ..Default::default()
        },
        AxiGdmaMem2MemRxStatus {
            descriptor_error: true,
            ..Default::default()
        },
        AxiGdmaMem2MemRxStatus {
            descriptor_empty: true,
            ..Default::default()
        },
    ];
    for rx in rx_failures {
        let status = AxiGdmaMem2MemStatus {
            rx,
            ..Default::default()
        };
        assert!(status.failed() && status.terminal(), "{rx:?}");
    }
    let status = AxiGdmaMem2MemStatus {
        tx: AxiGdmaMem2MemTxStatus {
            descriptor_error: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(status.failed() && status.terminal());
}

#[test]
fn progress_flags_alone_are_neither_failure_nor_end() {
    let status = AxiGdmaMem2MemStatus {
        rx: AxiGdmaMem2MemRxStatus {
            done: true,
            ..Default::default()
        },
        tx: AxiGdmaMem2MemTxStatus {
            done: true,
            eof: true,
            ..Default::default()
        },
    };
    assert!(!status.failed() && !status.terminal());
}

#[test]
fn fifo_events_are_observed_but_do_not_fail_a_transfer() {
    let status = AxiGdmaMem2MemStatus {
        rx: AxiGdmaMem2MemRxStatus {
            fifo_overflow: true,
            fifo_underflow: true,
            ..Default::default()
        },
        tx: AxiGdmaMem2MemTxStatus {
            fifo_overflow: true,
            fifo_underflow: true,
            ..Default::default()
        },
    };
    assert!(!status.failed() && !status.terminal());
}
