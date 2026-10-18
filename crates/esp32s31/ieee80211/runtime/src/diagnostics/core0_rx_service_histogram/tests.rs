use super::*;

#[test]
fn histogram_snapshot_uses_wrapping_deltas() {
    let mut earlier = Core0RxServiceHistogramSnapshot::default();
    earlier.bins[3].services = u32::MAX;
    earlier.bins[3].setup = 40;
    let mut current = Core0RxServiceHistogramSnapshot::default();
    current.bins[3].services = 1;
    current.bins[3].setup = 75;
    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(delta.bins[3].services, 2);
    assert_eq!(delta.bins[3].setup, 35);
}
