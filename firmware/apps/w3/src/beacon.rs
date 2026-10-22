//! W3's six-second software beacon-loss policy on a wrapping millisecond clock.
//! A run is bounded to minutes, well below the u32 clock's wrap period.
pub const fn lost(last_ms: u32, now_ms: u32) -> bool {
    now_ms.wrapping_sub(last_ms) >= 6000
}
#[cfg(test)]
mod tests {
    use super::*;
    use oer_time::{Clock, Duration, Instant};
    use oer_time_virtual::VirtualClock;
    #[test]
    fn six_seconds_without_a_beacon_including_clock_wrap() {
        let clock =
            VirtualClock::<1>::starting_at(Instant::from_micros(u64::from(u32::MAX) * 1000));
        let ms = || (clock.now().as_micros() / 1000) as u32;
        let last = ms();
        clock
            .advance(Duration::from_millis(5999))
            .expect("virtual time");
        assert!(!lost(last, ms()));
        clock
            .advance(Duration::from_millis(1))
            .expect("virtual time");
        assert!(lost(last, ms()));
        assert!(!lost(ms(), ms()));
    }
}
