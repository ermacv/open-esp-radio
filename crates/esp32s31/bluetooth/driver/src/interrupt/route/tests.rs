use super::{CpuInterruptRoutePolicy, CpuInterruptSource, InterruptHandlerResidency};

#[test]
fn primary_route_is_source_124_level_three_and_iram() {
    let policy = CpuInterruptRoutePolicy::PRIMARY;
    assert_eq!(policy.source(), CpuInterruptSource::PrimaryBtMac);
    assert_eq!(policy.source().number(), 124);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(policy.residency(), InterruptHandlerResidency::IramRequired);
    assert!(policy.pinned_to_controller_core());
}

#[test]
fn nrt_route_is_distinct_source_133_without_iram_request() {
    let policy = CpuInterruptRoutePolicy::NRT;
    assert_eq!(policy.source(), CpuInterruptSource::NrtBtMacInt1);
    assert_eq!(policy.source().number(), 133);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(
        policy.residency(),
        InterruptHandlerResidency::IramNotRequested
    );
    assert!(policy.pinned_to_controller_core());
}

#[test]
fn modem_lp_timer_route_is_source_127_level_three_and_iram() {
    let policy = CpuInterruptRoutePolicy::MODEM_LP_TIMER;
    assert_eq!(policy.source(), CpuInterruptSource::ModemLpTimer);
    assert_eq!(policy.source().number(), 127);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(policy.residency(), InterruptHandlerResidency::IramRequired);
    assert!(policy.pinned_to_controller_core());
}
