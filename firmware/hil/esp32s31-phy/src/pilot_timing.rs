//! Image-only samples of the public TIMG0 service call (not private SAR atoms).
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(any(target_arch = "riscv32", test))]
use oer_memory::AffineSpscQueue;
#[cfg(target_arch = "riscv32")]
use oer_memory::AffineSpscReceiver;
use oer_memory::AffineSpscSender;

/// Bounded per-root window; full queues retain earlier samples and count loss.
#[cfg(target_arch = "riscv32")]
pub const CAPACITY: usize = 4096;
#[cfg(target_arch = "riscv32")]
pub type Queue = AffineSpscQueue<Sample, CAPACITY>;
#[cfg(target_arch = "riscv32")]
pub type Sender = AffineSpscSender<'static, Sample, CAPACITY>;
#[cfg(target_arch = "riscv32")]
pub type Reader = AffineSpscReceiver<'static, Sample, CAPACITY>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    pub entry: u64,
    pub call: u64,
    pub returned: u64,
    pub empty: u64,
}
impl Sample {
    /// Counter rollover wraps; intervals below the measured empty-pair cost
    /// have no resolved positive duration and are reported as zero.
    pub fn adjusted(self) -> [u64; 2] {
        [
            self.call
                .wrapping_sub(self.entry)
                .saturating_sub(self.empty),
            self.returned
                .wrapping_sub(self.call)
                .saturating_sub(self.empty),
        ]
    }
}

pub fn publish<const N: usize>(
    sender: &AffineSpscSender<'_, Sample, N>,
    sample: Sample,
    lost: &AtomicU32,
) {
    if sender.try_send(sample).is_err() {
        // Only the ISR increments, the task reads after disabling the window.
        let _ = lost.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }
}

#[cfg(target_arch = "riscv32")]
pub async fn report(reader: &Reader, tick: u32, lost: u32) {
    use oer_diagnostics::Note;
    // SOURCE: firmware/bootstrap/esp32s31/src/main.rs selects Mhz160;
    // boot::CpuProfile::clock sources its PLL/dividers from IDF rtc_clk.c.
    const CPU_HZ: u32 = oer_esp32s31_hal::boot::CpuProfile::Mhz160
        .clock()
        .rates
        .cpu_hz;
    let console = &crate::transport::CONSOLE;
    let mut count = 0u32;
    while let Ok(sample) = reader.try_receive() {
        count = count.saturating_add(1);
        let [entry_to_call, service] = sample.adjusted();
        console
            .note_reliably(&Note::pilot_service(
                tick,
                count,
                entry_to_call,
                service,
                sample.empty,
            ))
            .await;
    }
    console
        .note_reliably(&Note::pilot_window(
            tick,
            count,
            lost,
            CPU_HZ,
            CAPACITY as u32,
        ))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_buffer_preserves_samples_and_counts_each_lost_event() {
        let mut queue = AffineSpscQueue::<Sample, 1>::new();
        let (sender, reader) = queue.split();
        let lost = AtomicU32::new(0);
        let first = Sample {
            entry: 1,
            call: 2,
            returned: 7,
            empty: 1,
        };
        publish(&sender, first, &lost);
        publish(&sender, Sample { entry: 20, ..first }, &lost);
        assert_eq!(reader.try_receive(), Ok(first));
        assert_eq!(lost.load(Ordering::Relaxed), 1);
        publish(&sender, first, &lost);
        assert_eq!(reader.try_receive(), Ok(first));
        assert_eq!(lost.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn empty_pair_is_subtracted_without_underflow_and_counter_wraps() {
        assert_eq!(
            Sample {
                entry: 100,
                call: 102,
                returned: 110,
                empty: 3
            }
            .adjusted(),
            [0, 5]
        );
        assert_eq!(
            Sample {
                entry: u64::MAX - 3,
                call: 2,
                returned: 9,
                empty: 2
            }
            .adjusted(),
            [4, 5]
        );
    }
}
