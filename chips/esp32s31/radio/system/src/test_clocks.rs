//! A backend for host domains that never acquire platform clocks.

use oer_esp32s31_hal::power::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
};

pub(crate) struct TestClockBackend;

impl PlatformClockBackend for TestClockBackend {
    fn acquire(_: PlatformClock) -> Result<(), PlatformClockError> {
        panic!("host domain must not acquire platform clocks");
    }
    fn release(_: PlatformClock) -> Result<(), PlatformClockError> {
        panic!("host domain must not release platform clocks");
    }
    fn abandon(_: PlatformClock, _: PlatformClockAbandonment) {
        panic!("host domain must not abandon platform clocks");
    }
}
