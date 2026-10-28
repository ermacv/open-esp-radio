//! The HIL service the lower-MAC conformance image mounts, and the key set
//! it reports: the union of the keys of that service and of the base module.

use oer_hil_protocol::lower_mac::{Prepare, RunCase};

oer_hil_agent::requests! {
    /// The conformance suite's preparation and case requests.
    pub(crate) enum Request(sessions = false) { Prepare(Prepare), Run(RunCase), }
}

oer_hil_agent::image_keys! {
    /// The lower-MAC conformance image.
    pub(crate) KEYS = [Request]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_image_reports_the_keys_its_feature_reported() {
        let old = oer_hil_protocol::feature_keys::image_keys(&|feature| {
            feature == "diagnostic-lower-mac"
        });
        assert_eq!(super::KEYS.hello(0), old.as_set().hello(0));
        assert_eq!(super::KEYS.page(0), old.as_set().page(0));
    }
}
