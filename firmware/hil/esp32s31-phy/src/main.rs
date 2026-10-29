#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#[cfg(any(target_arch = "riscv32", test))]
mod ieee802154_fault;
#[cfg(any(test, all(target_arch = "riscv32", oer_trace)))]
mod timer_measurements;
#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(all(test, not(target_arch = "riscv32")))]
mod cache;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod completion;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod hold;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod pilot_timing;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod services;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn boot_entropy_reads_follow_clock_publication() {
        // The target entry cannot execute on the host. Its linear startup
        // must publish the clock before RNG's second read waits on that clock.
        let entry = include_str!("entry.rs");
        let publish = entry
            .find("oer_espressif_executor::init::<Esp32s31, 2")
            .expect("the image publishes its system counter");
        let sample = entry
            .find("rng.random()")
            .expect("the boot identity draws entropy");
        assert!(publish < sample, "an unpublished clock stalls boot entropy");
    }
}
