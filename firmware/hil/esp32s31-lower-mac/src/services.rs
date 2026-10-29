//! The HIL service the lower-MAC conformance image mounts, and the key set
//! it reports: the union of the keys of that service and of the base module.

use oer_ieee80211_conformance_control::{Prepare, RunCase};

oer_hil_agent::requests! {
    /// The conformance suite's preparation and case requests.
    pub(crate) enum Request(sessions = false) { Prepare(Prepare), Run(RunCase),
        Describe(oer_hil_protocol::capture::Describe),
        Counters(oer_hil_protocol::capture::ReadCounters), }
    publishes [oer_hil_protocol::capture::Refused]
}

oer_hil_agent::image_keys! {
    /// The lower-MAC conformance image.
    pub(crate) KEYS = [Request]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use oer_hil_agent::base::{Base, Service};
    use oer_hil_protocol::{Key, Message};
    use oer_ieee80211_conformance_control as lower_mac;

    #[test]
    fn the_image_reports_its_service_and_the_base_module() {
        let reported: BTreeSet<Key> = super::KEYS.keys().iter().copied().collect();
        let expected: BTreeSet<Key> = Base::KEYS
            .iter()
            .chain(lower_mac::KEYS)
            .chain(
                [
                    oer_hil_protocol::capture::Describe::KEY,
                    oer_hil_protocol::capture::Description::KEY,
                    oer_hil_protocol::capture::ReadCounters::KEY,
                    oer_hil_protocol::capture::Counters::KEY,
                    oer_hil_protocol::capture::Refused::KEY,
                ]
                .iter(),
            )
            .copied()
            .collect();
        assert_eq!(reported, expected);
    }
}
