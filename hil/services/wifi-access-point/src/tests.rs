extern crate std;

use super::*;

#[test]
fn the_longest_serve_fits_one_request() {
    let serve = Serve {
        ssid: heapless::String::try_from("s".repeat(MAX_SSID).as_str()).unwrap(),
        passphrase: heapless::String::try_from("p".repeat(MAX_PASSPHRASE).as_str()).unwrap(),
        channel: 13,
        clients: 4,
    };
    let mut buffer = [0; 256];
    let encoded = postcard::to_slice(&serve, &mut buffer).unwrap();
    assert!(encoded.len() <= 128, "{} bytes", encoded.len());
}

#[test]
fn the_service_names_three_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_protocols_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 6);
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len());
}
