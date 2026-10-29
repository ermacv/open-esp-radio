//! Start the noise source and its health tests without waiting for them.
//! SOURCE: ESP-IDF 4d80c1b2 `hal/esp32s31/include/hal/rng_ll.h:163-178`.
use crate::Access;

pub struct Profile<N, C> {
    pub noise_source: N,
    pub noise_position: N,
    pub repetition_cutoff: C,
    pub adaptive_cutoff: C,
    pub startup_samples: u16,
}

pub trait Registers {
    type Noise: Copy;
    type Cutoff: Copy;
    fn open_bus_clock(&mut self, access: Access<'_>);
    fn open_clock(&mut self, access: Access<'_>);
    fn reset(&mut self, access: Access<'_>, asserted: bool);
    fn configure_noise(&mut self, access: Access<'_>, profile: &Profile<Self::Noise, Self::Cutoff>);
    fn configure_health_test(&mut self, access: Access<'_>, samples: u16);
    fn enable_sampling(&mut self, access: Access<'_>);
    fn start_health_test(&mut self, access: Access<'_>);
}

pub fn start<R: Registers>(registers: &mut R, profile: &Profile<R::Noise, R::Cutoff>) {
    registers.open_bus_clock(Access::new());
    registers.open_clock(Access::new());
    registers.reset(Access::new(), true);
    registers.reset(Access::new(), false);
    registers.configure_noise(Access::new(), profile);
    registers.configure_health_test(Access::new(), profile.startup_samples);
    registers.enable_sampling(Access::new());
    registers.start_health_test(Access::new());
}
