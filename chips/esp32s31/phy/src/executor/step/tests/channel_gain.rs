use super::*;
use crate::channel::{
    PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
    PhyWifiTxGainImage,
};

impl Io {
    pub(in crate::executor::step) fn channel_gain_base(&mut self) -> Result<u8, Failure> {
        self.atom();
        self.channel_base_reads += 1;
        Ok(0xf3)
    }
}

fn image() -> PhyWifiTxGainImage {
    let mut image = PhyWifiTxGainImage {
        seed: [0; 6],
        output_32: [0; 8],
        output_64: [0; 16],
        output_72: [0; 16],
        config: 42,
    };
    for index in 0..32 {
        image.output_32[index / 4] |= (32 + index as u32) << ((index % 4) * 8);
        image.output_72[index / 2] |= (1000 + index as u32) << ((index % 2) * 16);
    }
    image
}

fn binding() -> Option<PhyChipChannelExternalBinding> {
    Some(
        PhyChipChannelExternalBinding::lower(PhyChipChannelAction::PublishTxGain(image()))
            .expect("publication binding"),
    )
}

#[test]
fn all_entries_cross_poll_boundaries_in_source_order_with_one_base_read() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut binding = binding();
    let mut completion = None;
    let mut timing = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::channel(&mut binding, &mut completion, clock.now()).expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &clock,
            Some(&mut timing)
        ));
        let mut polls = 0;
        loop {
            let before = atoms.get();
            let result = future.as_mut().poll(&mut cx);
            assert!(
                atoms.get() - before <= if polls == 0 { 9 } else { 8 },
                "at most eight whole entries and the initial base read"
            );
            polls += 1;
            if let Poll::Ready(result) = result {
                result.expect("publication completes");
                break;
            }
            assert!(
                polls < 10,
                "publication is finite and checkpoints its groups"
            );
            clock.advance(Duration::from_micros(1)).expect("clock fits");
        }
        assert!(polls > 2, "the table cannot be published in one poll");
    }
    assert!(binding.is_none());
    assert_eq!(completion, Some(PhyChipChannelCompletion::TxGainPublished));
    assert_eq!(io.channel_base_reads, 1);
    let expected: Vec<_> = (0_u8..32)
        .map(|i| {
            oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
                0xf3_u8.wrapping_add(i),
                1000 + u16::from(i),
                0,
                32 + i,
                [0; 4],
                42,
            )
        })
        .collect();
    assert_eq!(io.power_gain_entries, expected);
    assert!(!timing.invalid);
}

#[test]
fn drop_or_failure_leaves_the_partial_image_without_a_completion() {
    for failure in [None, Some(5)] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io {
            power_gain_failure: failure,
            ..Io::default()
        };
        let mut binding = binding();
        let mut completion = None;
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut job =
                Job::channel(&mut binding, &mut completion, clock.now()).expect("clock fits");
            let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
                &mut job, &mut io, &clock, None
            ));
            let result = future.as_mut().poll(&mut cx);
            if failure.is_some() {
                assert_eq!(result, Poll::Ready(Err(Failure::Binding)));
            } else {
                assert!(result.is_pending());
            }
        }
        assert!(completion.is_none());
        assert!(binding.is_some());
        assert_eq!(io.power_gain_entries.len(), failure.unwrap_or(8));
        assert_eq!(io.channel_base_reads, 1);
    }
}

#[test]
fn maximum_poll_and_atom_retain_their_own_group_or_terminal_identity() {
    let mut cursor = super::super::channel_gain::Cursor::default();
    // Advance through real entry groups; observations use a hermetic synthetic clock record.
    let mut io = Io::default();
    let mut target = super::super::io::Io::Test(&mut io, core::marker::PhantomData);
    cursor.advance(&mut target, &image()).expect("first group");
    let mut timing = EngineTiming {
        maximum_micros: 30,
        maximum_ready_micros: 20,
        ..EngineTiming::default()
    };
    cursor.observe(&timing, false);
    assert_eq!(cursor.maximum_entries(), [8, 8]);
    cursor.advance(&mut target, &image()).expect("second group");
    timing.maximum_micros = 32;
    cursor.observe(&timing, false);
    assert_eq!(cursor.maximum_entries(), [16, 8]);
    timing.maximum_micros = 33;
    cursor.observe(&timing, true);
    assert_eq!(cursor.maximum_entries(), [255, 8]);
}
