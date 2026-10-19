use super::{MacColdHandshakeBackend, MacColdHandshakeTimeout, execute_cold_mac_handshake};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandshakeEvent {
    Request,
    Sample,
    MaskInterrupts,
    ClearInterrupts,
}

struct HandshakeBackend {
    ready_after: u32,
    observations: u32,
    events: Vec<HandshakeEvent>,
}

impl HandshakeBackend {
    fn new(ready_after: u32) -> Self {
        Self {
            ready_after,
            observations: 0,
            events: Vec::new(),
        }
    }
}

impl MacColdHandshakeBackend for HandshakeBackend {
    fn request_cold_start(&mut self) {
        self.events.push(HandshakeEvent::Request);
    }

    fn sample_cold_start_ready(&mut self) -> bool {
        self.events.push(HandshakeEvent::Sample);
        let ready = self.observations == self.ready_after;
        self.observations += 1;
        ready
    }

    fn mask_mac_interrupts(&mut self) {
        self.events.push(HandshakeEvent::MaskInterrupts);
    }

    fn clear_mac_interrupts(&mut self) {
        self.events.push(HandshakeEvent::ClearInterrupts);
    }
}

#[test]
fn cold_handshake_polls_then_masks_and_clears_interrupts() {
    let mut backend = HandshakeBackend::new(2);
    let outcome = execute_cold_mac_handshake(&mut backend, 4).unwrap();

    assert_eq!(outcome.samples, 2);
    assert_eq!(outcome.observations, 3);
    assert_eq!(
        backend.events,
        [
            HandshakeEvent::Request,
            HandshakeEvent::Sample,
            HandshakeEvent::Sample,
            HandshakeEvent::Sample,
            HandshakeEvent::MaskInterrupts,
            HandshakeEvent::ClearInterrupts,
        ]
    );
}

#[test]
fn cold_handshake_timeout_stops_before_interrupt_cleanup() {
    let mut backend = HandshakeBackend::new(u32::MAX);
    let error = execute_cold_mac_handshake(&mut backend, 2).unwrap_err();

    assert_eq!(
        error,
        MacColdHandshakeTimeout {
            samples: 2,
            sample_limit: 2,
        }
    );
    assert_eq!(
        backend.events,
        [
            HandshakeEvent::Request,
            HandshakeEvent::Sample,
            HandshakeEvent::Sample,
        ]
    );
}

#[test]
fn cold_handshake_samples_ready_once_with_zero_not_ready_budget() {
    let mut backend = HandshakeBackend::new(0);
    let outcome = execute_cold_mac_handshake(&mut backend, 0).unwrap();

    assert_eq!(outcome.samples, 0);
    assert_eq!(outcome.observations, 1);
    assert_eq!(
        backend.events,
        [
            HandshakeEvent::Request,
            HandshakeEvent::Sample,
            HandshakeEvent::MaskInterrupts,
            HandshakeEvent::ClearInterrupts,
        ]
    );
}

#[test]
fn cold_handshake_zero_limit_times_out_after_the_initial_not_ready_sample() {
    let mut backend = HandshakeBackend::new(u32::MAX);
    let error = execute_cold_mac_handshake(&mut backend, 0).unwrap_err();

    assert_eq!(
        error,
        MacColdHandshakeTimeout {
            samples: 1,
            sample_limit: 0,
        }
    );
    assert_eq!(
        backend.events,
        [HandshakeEvent::Request, HandshakeEvent::Sample]
    );
}
