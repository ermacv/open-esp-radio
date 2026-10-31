//! Family cold-start order. Chip bindings supply owners, profiles and leaf
//! adapters; images supply only their payload and flash-tuning resources.
use oer_espressif_boot::{
    cache, clock,
    drivers::{self, access_control, cores, entropy, system_timer::LatchStuck},
    flash, mmu, psram,
    wait::{BootBudget, BootDeadline, BootWait},
};
use oer_espressif_platform_port::EarlyText;
use oer_espressif_staged_layout::Layout;
use oer_time::Clock;

/// CPU settings expressed in the register value types of one chip binding.
pub type CpuConfiguration<B> = clock::CpuClockProfile<
    <<B as BootProfile>::Clock as clock::CpuClockRegisters>::Root,
    <<B as BootProfile>::Clock as clock::CpuClockRegisters>::Dividers,
    <<B as BootProfile>::Clock as clock::PllRegisters>::Target,
>;

/// The chip's binding to the family cold-start sequence. No method chooses
/// cross-block startup order; that order belongs to `run`.
pub trait BootProfile: Sized {
    type Port: EarlyText;
    type Root;
    type Console;
    type Timer;
    type Monotonic: Clock;
    type Soc: cores::Registers + access_control::Registers + entropy::Registers;
    type Brownout;
    type LpFast: drivers::clock::LpFastRegisters;
    type Clock: clock::CpuClockRegisters
        + clock::ClockUpdate
        + clock::PllRegisters
        + clock::PllPower
        + psram::MemoryClockRegisters
        + flash::FlashClockRegisters;
    type Active: clock::ActivePllPower;
    type Psram: psram::PsramMemory;
    type Cache: cache::CacheSyncRegisters
        + cache::FetchSuspendRegisters
        + cache::SecondaryFetchRegisters;
    type Flash: flash::FlashCommandRegisters + flash::FlashTimingRegisters + mmu::MmuRegisters;
    type WatchdogParts;
    type Watchdogs;

    const BUDGET: BootBudget;
    const LP_FAST: drivers::clock::LpFastProfile<
        <Self::LpFast as drivers::clock::LpFastRegisters>::Source,
    >;
    const ENTROPY: entropy::Profile<
        <Self::Soc as entropy::Registers>::Noise,
        <Self::Soc as entropy::Registers>::Cutoff,
    >;
    const CPU: CpuConfiguration<Self>;
    const CLOCK_WAITS: clock::ClockWaits;
    const PSRAM_WAITS: psram::PsramWaits;
    const CACHE: cache::CacheGeometry<<Self::Cache as cache::CacheSyncRegisters>::Map>;
    const CACHE_SYNC: BootWait;
    const TUNING_WAITS: flash::TuningWaits;
    /// The waits of the suspension and resumption that open a secondary
    /// hart's instruction bus.
    const SECONDARY_FETCH_WAITS: cache::SuspendWaits;
    fn timings() -> &'static [<Self::Flash as flash::FlashTimingRegisters>::Timing];
    fn psram_profile(
        size: u32,
    ) -> psram::OctalPsramProfile<<Self::Clock as psram::MemoryClockRegisters>::SupplyLevel>;
    fn flash_geometry(capacity: u32) -> flash::FlashGeometry;

    fn claim() -> Option<Self::Root>;
    fn split(root: Self::Root) -> Owners<Self>;
    fn install_console(console: Self::Console) -> Result<(), &'static str>;
    /// Open the owned timer clock, then call the family's named
    /// `Monotonic::start_initial_latch_check`, before a deadline exists.
    fn start_timer(timer: Self::Timer) -> Result<Self::Monotonic, LatchStuck>;
    fn take_watchdogs(parts: Self::WatchdogParts) -> Self::Watchdogs;
    fn hand_off_watchdogs(watchdogs: Self::Watchdogs);
    fn brownout(
        owner: &mut Self::Brownout,
        deadline: &BootDeadline<'_, Self::Monotonic>,
    ) -> Result<(), drivers::brownout::BrownoutFailure>;
    /// Consume the successful affine witness through the HAL loan primitive.
    fn loan_psram(mapped: psram::MappedPsram<Self::Psram>) -> &'static mut [u8];
}

/// The root's unstarted parts. Construction itself performs no startup.
pub struct Owners<B: BootProfile> {
    pub console: B::Console,
    pub timer: B::Timer,
    pub soc: B::Soc,
    pub brownout: B::Brownout,
    pub lp_fast: B::LpFast,
    pub clock: B::Clock,
    pub active: B::Active,
    pub psram: B::Psram,
    pub cache: B::Cache,
    pub flash: B::Flash,
    pub flash_window: mmu::MmuWindow,
    pub watchdogs: B::WatchdogParts,
}

/// Image-owned flash payload, cold reference and fallibly claimed SRAM scratch.
pub struct Resources<'a, S> {
    pub layout: &'a Layout,
    pub payload: &'a [u8],
    pub tuning_reference: &'a [u32],
    pub flash_capacity: u32,
    pub scratch: S,
}

/// Run the unchanged cold start through publication and the final RTC disable.
/// `HARTS` is the runtime payload's used-hart mask
/// ([`runtime_harts`](crate::runtime_harts)): after the code is published the
/// bootstrap opens each used secondary hart's instruction bus.
#[cfg(target_arch = "riscv32")]
pub fn run<B: BootProfile, const ENTRY: usize, const HARTS: u32>(
    resources: Resources<'_, impl FnOnce() -> Option<&'static mut [u32]>>,
) -> !
where
    <B::Flash as flash::FlashTimingRegisters>::Timing: 'static,
{
    use crate::{fail, print};
    let root = B::claim();
    crate::start::<B::Port>();
    let root =
        root.unwrap_or_else(|| fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=root-taken\r\n"));
    let Owners {
        console,
        timer,
        mut soc,
        mut brownout,
        mut lp_fast,
        mut clock,
        mut active,
        psram,
        mut cache,
        mut flash,
        flash_window,
        watchdogs,
    } = B::split(root);
    B::install_console(console).unwrap_or_else(|reason| {
        print::<B::Port>("OER_BOOT bootstrap=FAIL reason=");
        print::<B::Port>(reason);
        fail::<B::Port>("\r\n")
    });
    let monotonic = B::start_timer(timer).unwrap_or_else(|_| {
        print::<B::Port>("OER_BOOT bootstrap=FAIL reason=");
        print::<B::Port>("monotonic");
        fail::<B::Port>("\r\n")
    });
    let deadline = BootDeadline::start(&monotonic, B::BUDGET);
    checked::<B, _>(drivers::clock::select_lp_fast(
        &mut lp_fast,
        B::LP_FAST,
        &deadline,
    ));
    drivers::soc::initialize(&mut soc, &B::ENTROPY, &deadline)
        .unwrap_or_else(|_| fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=soc-init\r\n"));
    let watchdogs = B::take_watchdogs(watchdogs);
    B::brownout(&mut brownout, &deadline)
        .unwrap_or_else(|_| fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=brownout-policy\r\n"));
    print::<B::Port>("OER_BOOT bootstrap=BROWNOUT\r\n");
    let _rates = checked::<B, _>(clock::bring_up(
        &mut clock,
        &mut active,
        B::CPU,
        B::CLOCK_WAITS,
        &deadline,
    ));
    print::<B::Port>("OER_BOOT bootstrap=INIT\r\n");
    let mapped = psram::bring_up(
        psram::PsramOwners {
            clock: &mut clock,
            active: &mut active,
            psram,
            cache: &mut cache,
        },
        B::psram_profile(resources.layout.psram.length()),
        B::CACHE,
        B::PSRAM_WAITS,
        &deadline,
    )
    .unwrap_or_else(|error| crate::fail_boot::<B::Port>(error.failure()));
    let memory = B::loan_psram(mapped);
    if memory.as_ptr() as usize != resources.layout.psram.origin() as usize {
        fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=psram-init\r\n");
    }
    let image = crate::stage(
        resources.layout,
        resources.payload,
        memory,
        |progress| match progress {
            crate::StageProgress::Psram => {
                print::<B::Port>("OER_BOOT bootstrap=PSRAM\r\n");
                let source_address = resources.payload.as_ptr() as usize;
                let source_end = source_address
                    .checked_add(resources.payload.len())
                    .unwrap_or_else(|| {
                        fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=source-overflow\r\n")
                    });
                let xip = resources.layout.flash_xip;
                if !(xip.origin() as usize..xip.end() as usize).contains(&source_address)
                    || source_end > xip.end() as usize
                {
                    fail::<B::Port>("OER_BOOT bootstrap=FAIL reason=source-not-xip\r\n");
                }
            }
            crate::StageProgress::SourceCrc => {
                print::<B::Port>("OER_BOOT bootstrap=SOURCE_CRC\r\n")
            }
            crate::StageProgress::DestinationCrc => {
                print::<B::Port>("OER_BOOT bootstrap=DESTINATION_CRC\r\n")
            }
        },
    )
    .unwrap_or_else(|error| crate::fail_stage::<B::Port>(error));
    let tuning_virtual = resources.tuning_reference.as_ptr() as u32;
    let tuning_physical = checked::<B, _>(mmu::physical_address(
        &mut flash,
        flash_window,
        mmu::MmuTarget::Flash,
        tuning_virtual,
    ));
    let scratch =
        checked::<B, _>((resources.scratch)().ok_or(flash::FlashFailure::ScratchClaimed.into()));
    checked::<B, _>(flash::tune(
        flash::TuningOwners {
            flash: &mut flash,
            clock: &mut clock,
            cache: &mut cache,
            xip: &Reference(resources.tuning_reference),
        },
        B::flash_geometry(resources.flash_capacity),
        flash::TuningRegion {
            physical_start: tuning_physical,
            virtual_start: tuning_virtual,
            bytes: core::mem::size_of_val(resources.tuning_reference) as u32,
        },
        B::timings(),
        scratch,
        B::TUNING_WAITS,
        &deadline,
    ));
    print::<B::Port>("OER_BOOT bootstrap=FLASH_TUNED\r\n");
    let published = checked::<B, _>(cache::publish_code(
        &mut cache,
        B::CACHE,
        image.load_address() as u32,
        image.payload_len() as u32,
        B::CACHE_SYNC,
        &deadline,
    ));
    checked::<B, _>(cache::open_secondary_fetch(
        &mut cache,
        HARTS,
        B::SECONDARY_FETCH_WAITS,
        &deadline,
    ));
    crate::hand_off::<B::Port, ENTRY>(image, published, || B::hand_off_watchdogs(watchdogs))
}

#[cfg(target_arch = "riscv32")]
fn checked<B: BootProfile, T>(result: Result<T, oer_espressif_boot::BootFailure>) -> T {
    result.unwrap_or_else(|error| crate::fail_boot::<B::Port>(error))
}

#[cfg(any(test, target_arch = "riscv32"))]
struct Reference<'a>(&'a [u32]);
#[cfg(any(test, target_arch = "riscv32"))]
impl flash::XipWindow for Reference<'_> {
    fn word(&self, index: usize) -> Result<u32, flash::FlashFailure> {
        self.0
            .get(index)
            .map(oer_memory::read_volatile)
            .ok_or(flash::FlashFailure::XipOutside {
                index,
                words: self.0.len(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xip_reads_return_the_exact_rejected_index_and_region_length() {
        use oer_espressif_boot::flash::XipWindow;

        let words = [0x1234_5678, 0x89ab_cdef];
        let region = Reference(&words);
        assert_eq!(region.word(0), Ok(words[0]));
        assert_eq!(region.word(1), Ok(words[1]));
        for index in [words.len(), usize::MAX] {
            assert_eq!(
                region.word(index),
                Err(flash::FlashFailure::XipOutside {
                    index,
                    words: words.len(),
                })
            );
        }
        assert_eq!(
            Reference(&[]).word(0),
            Err(flash::FlashFailure::XipOutside { index: 0, words: 0 })
        );
    }
}
