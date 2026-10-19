#![no_std]
use oer_ieee80211_arbiter::{
    Arbiter, ArbiterEnd, ChannelRequest, FailedArbiter, LeaseId, LeasePlace, PendingRetune, UpstreamFact,
};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_time::Instant;

pub fn start<'id, K: Ieee80211Capabilities>(mut arbiter: Arbiter<'id, K>) {
    let mut failed = arbiter.finish(ArbiterEnd::PortFailed);
    #[cfg(ui_reject)]
    failed.want_access_point(); // ui-error: E0599
    drop(failed.poll_notice());
}

pub fn upstream<'id, K: Ieee80211Capabilities>(
    mut failed: FailedArbiter<'id, K>,
    now: Instant,
    fact: UpstreamFact<K>,
) {
    #[cfg(ui_reject)]
    failed.upstream(now, fact); // ui-error: E0599
    drop(failed.poll_notice());
}

pub fn retune<'id, K: Ieee80211Capabilities>(
    mut failed: FailedArbiter<'id, K>,
    pending: PendingRetune<'id, K>,
) {
    #[cfg(ui_reject)]
    failed.retuned(pending); // ui-error: E0599
    #[cfg(not(ui_reject))]
    failed.retune_ended(pending);
}

pub fn request<'id, K: Ieee80211Capabilities>(
    mut failed: FailedArbiter<'id, K>,
    now: Instant,
    request: ChannelRequest<K>,
    place: LeasePlace<'id>,
) {
    #[cfg(ui_reject)]
    drop(failed.request(now, request, place)); // ui-error: E0599
    drop(failed.poll_notice());
}

pub fn ready<'id, K: Ieee80211Capabilities>(
    mut failed: FailedArbiter<'id, K>,
    lease: LeaseId<'id>,
) {
    #[cfg(ui_reject)]
    failed.ready(lease); // ui-error: E0599
    drop(failed.poll_notice());
}

pub fn restart<'id, K: Ieee80211Capabilities>(
    mut failed: FailedArbiter<'id, K>,
    channel: oer_ieee80211_lower_mac::SupportedChannel<K>,
) {
    #[cfg(ui_reject)]
    drop(failed.restart(channel)); // ui-error: E0599
    drop(failed.poll_notice());
}
