use super::*;
use crate::{Envelope, FrameDecoder, FrameEncoder, Message, WireKind};
fn roundtrip<M: Message + Clone + core::fmt::Debug + Eq>(body: M) {
    let mut encoder = FrameEncoder::new();
    let envelope = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, body.clone());
    let bytes = encoder
        .encode(&envelope)
        .expect("worst-case page fits existing frame");
    let mut decoder = FrameDecoder::new();
    let mut decoded = None;
    decoder.feed(WireKind::Event, bytes, |frame| {
        decoded = Some(
            frame
                .expect("frame")
                .decode::<M>()
                .expect("key")
                .expect("payload")
                .body,
        );
    });
    assert_eq!(decoded, Some(body));
}
#[test]
fn maximum_pages_roundtrip_without_enlarging_the_transport() {
    let identity = HealthIdentity {
        observation: u32::MAX,
        epoch: u32::MAX,
    };
    let read = Bracket {
        first_micros: u64::MAX,
        last_micros: u64::MAX,
    };
    let counter = Counter {
        cut: Cut::After,
        epoch: u32::MAX,
        read,
        kind: CounterKind::TxTrcts,
        value: CounterValue::SignedScaled40(i32::MIN),
    };
    roundtrip(CounterPage {
        identity,
        first: u16::MAX,
        total: u16::MAX,
        records: heapless::Vec::from_slice(&[counter; COUNTER_PAGE]).expect("page"),
    });
    let signal = Signal {
        read,
        channel: u8::MAX,
        gain: Some(u16::MAX),
        value: SignalValue::Temperature(Temperature {
            sensor_units: i16::MIN,
            acquisition: Acquisition::Window(read),
        }),
    };
    roundtrip(SignalPage {
        identity,
        first: u16::MAX,
        total: u16::MAX,
        records: heapless::Vec::from_slice(&[signal; SIGNAL_PAGE]).expect("page"),
    });
    let tracking = Tracking {
        condition: None,
        operation: TrackingOperation::Other(u8::MAX),
        due: Due::Unknown,
        disposition: Disposition::Failed,
        at_micros: u64::MAX,
        committed: u8::MAX,
    };
    roundtrip(TrackingPage {
        identity,
        first: u16::MAX,
        total: u16::MAX,
        records: heapless::Vec::from_slice(&[tracking; DETAIL_PAGE]).expect("page"),
    });
    let context = MeasurementContext {
        gain: u8::MAX,
        measurement: u8::MAX,
        shared_radio: true,
        radio_stage: true,
        fine_code: Some(u8::MAX),
        high_level: true,
        frequency_code: Some(u16::MAX),
    };
    for value in [
        MeasurementValue::DcIqCaptured {
            request: DcIqRequest {
                context,
                iteration: u8::MAX,
                chain: u8::MAX,
                control: u16::MAX,
                mode: u8::MAX,
            },
            sums: Some([i32::MIN; 3]),
        },
        MeasurementValue::RxDcCorrection {
            context,
            configuration: [u16::MAX; 2],
            iterations: u8::MAX,
            converged: false,
            residual: Some([i32::MIN; 3]),
        },
    ] {
        let measurement = Measurement {
            read,
            channel: Some(u8::MAX),
            gain: Some(u16::MAX),
            value,
        };
        roundtrip(MeasurementPage {
            identity,
            first: u16::MAX,
            total: u16::MAX,
            records: heapless::Vec::from_slice(&[measurement; DETAIL_PAGE]).expect("page"),
        });
    }
}
#[test]
fn modulo_deltas_require_matching_widths_and_continuity() {
    assert_eq!(
        CounterValue::Modulo10(3).delta_since(CounterValue::Modulo10(1022), true),
        Some(5)
    );
    assert_eq!(
        CounterValue::Modulo16(3).delta_since(CounterValue::Modulo16(65534), true),
        Some(5)
    );
    assert_eq!(
        CounterValue::Modulo32(3).delta_since(CounterValue::Modulo32(u32::MAX - 1), true),
        Some(5)
    );
    assert_eq!(
        CounterValue::Modulo10(3).delta_since(CounterValue::Modulo10(1022), false),
        None
    );
    assert_eq!(
        CounterValue::Modulo10(1024).delta_since(CounterValue::Modulo10(0), true),
        None
    );
    assert_eq!(
        CounterValue::Modulo16(3).delta_since(CounterValue::Modulo10(3), true),
        None
    );
    assert_eq!(
        CounterValue::SignedScaled40(3).delta_since(CounterValue::SignedScaled40(1), true),
        None
    );
}
#[test]
fn inspecting_retained_temperature_cannot_make_it_fresh() {
    let t = Temperature {
        sensor_units: 4,
        acquisition: Acquisition::Unobserved,
    };
    assert_eq!(t.age_micros(100), None);
    let t = Temperature {
        acquisition: Acquisition::Window(Bracket {
            first_micros: 10,
            last_micros: 20,
        }),
        ..t
    };
    assert_eq!(t.age_micros(100), Some(90));
    assert_eq!(t.age_micros(19), None);
    let t = Temperature {
        acquisition: Acquisition::Window(Bracket {
            first_micros: 20,
            last_micros: 10,
        }),
        ..t
    };
    assert_eq!(t.age_micros(100), None);
}
