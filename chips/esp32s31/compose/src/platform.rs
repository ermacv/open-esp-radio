//! S31 owners and platform operations used by image compositions.
//! The image selects entries, priorities, tasks and terminal policy.
use crate::Esp32s31;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{peripherals as hw, root};
use oer_espressif_interrupt_table::{Controller, Entry, InterruptSource, Router};
use oer_espressif_platform_port::AlarmPart;

/// Associated interrupt parts are supplied by the same platform that owns
/// their construction and service, rather than named by an image's HAL edge.
pub trait InterruptTypes {
    type AppWake: InterruptSource;
    type RadioWake: InterruptSource;
    type Doorbell: InterruptSource;
    type AppAlarm: AlarmPart;
    type RadioAlarm: AlarmPart;
    type Phy: InterruptSource;
    type Console: InterruptSource;
}
impl InterruptTypes for Esp32s31 {
    type AppWake = hw::software_interrupt::FromCpu0Irq;
    type RadioWake = hw::software_interrupt::FromCpu2Irq;
    type Doorbell = hw::software_interrupt::FromCpu3Irq;
    type AppAlarm = hw::system_timer::SystimerAlarm1;
    type RadioAlarm = hw::system_timer::SystimerAlarm0;
    type Phy = hw::timg0_alarm::Timg0AlarmInterrupt;
    type Console = hw::usb_serial_jtag::ConsoleInterrupt;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareError {
    Claimed,
    Initialization(oer_espressif_staged_runtime::adoption::Error),
}
/// Claim once and execute the shared runtime adoption driver. No image
/// sequence is copied here; mapping and image boot evidence remain explicit.
pub fn prepare() -> Result<oer_espressif_staged_runtime::adoption::Prepared<Esp32s31>, PrepareError>
{
    let root = root::Root::take().ok_or(PrepareError::Claimed)?;
    oer_espressif_staged_runtime::adoption::prepare::<Esp32s31>(root.into_runtime()).map_err(
        |error| {
            PrepareError::Initialization(
                oer_espressif_staged_runtime::adoption::Error::Initialization(error),
            )
        },
    )
}

/// Factory identity and interface addresses, with no HAL enum in callers.
pub struct DeviceIdentity(hw::efuse::FactoryIdentity);
impl DeviceIdentity {
    pub fn read(efuse: hw::efuse::EfuseRegisters) -> Self {
        Self(hw::efuse::FactoryIdentity::read(efuse))
    }
    pub fn station_address(&self) -> [u8; 6] {
        self.0.address(hw::efuse::Interface::Station)
    }
    pub fn ap_address(&self) -> [u8; 6] {
        self.0.address(hw::efuse::Interface::AccessPoint)
    }
    pub fn base_mac(&self) -> [u8; 6] {
        self.0.base_mac()
    }
    pub fn mac_extension(&self) -> u16 {
        self.0.mac_extension()
    }
}

/// The unique entropy owner moves from image seed generation to the radio.
pub struct Entropy(hw::entropy::Entropy<EmbassyClock>);
impl Entropy {
    pub fn new(registers: hw::entropy::EntropyRegisters, clock: EmbassyClock) -> Self {
        Self(hw::entropy::Entropy::new(registers, clock))
    }
    pub fn is_healthy(&self) -> bool {
        self.0.status().is_healthy()
    }
    pub fn random(&mut self) -> u32 {
        self.0.random()
    }
    pub fn fill(&mut self, bytes: &mut [u8]) {
        self.0.fill(bytes);
    }
}

/// Install the existing P2 writer and P1 comparator without changing routes,
/// and return the time driver's authority with the comparator left for an
/// executor of another hart (`oer_espressif_executor::grant`).
pub fn timers<const H: u8, A, R>(
    timer: hw::system_timer::SystemTimer,
    router: &mut Router<impl Controller, H>,
    app: A,
    radio: R,
) -> (
    oer_espressif_executor::TimeDriverReady<Esp32s31>,
    hw::system_timer::SystimerAlarm2,
)
where
    A: Entry<Part = hw::system_timer::SystimerAlarm1>,
    R: Entry<Part = hw::system_timer::SystimerAlarm0>,
{
    let mut timer_ready = oer_espressif_executor::init::<Esp32s31, 2, _, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        router,
        radio,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _, _>(
        &mut timer_ready,
        router,
        app,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    (timer_ready, timer.alarm2)
}

/// Bind a trace clock with `trace_clock!(platform::trace_now)` in an image
/// that declares the trace class. This function owns no recorder or counter.
#[cfg(oer_trace)]
pub fn trace_now() -> u64 {
    use oer_time::Clock;
    EmbassyClock.now().as_micros()
}
/// The image chooses this terminal path for an unanswered owned request.
pub fn reset_request<T>(error: oer_memory::RequestError) -> T {
    oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, T>(error)
}
