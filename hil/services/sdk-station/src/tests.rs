extern crate std;

use super::*;

/// The image's request payload bound.
const MAXIMUM_PAYLOAD_BYTES: usize = 128;

#[test]
fn the_longest_connect_and_exchange_fit_one_request() {
    let connect = Connect {
        ssid: heapless::String::try_from("s".repeat(MAX_SSID).as_str()).unwrap(),
        passphrase: heapless::String::try_from("p".repeat(MAX_PASSPHRASE).as_str()).unwrap(),
    };
    let exchange = Exchange {
        peer: [255; 4],
        port: u16::MAX,
        line: heapless::Vec::from_slice(&[0xff; MAX_LINE]).unwrap(),
        timeout_ms: u32::MAX,
    };
    let mut buffer = [0; 256];
    let connect = postcard::to_slice(&connect, &mut buffer).unwrap().len();
    assert!(connect <= MAXIMUM_PAYLOAD_BYTES, "{connect} bytes");
    let exchange = postcard::to_slice(&exchange, &mut buffer).unwrap().len();
    assert!(exchange <= MAXIMUM_PAYLOAD_BYTES, "{exchange} bytes");
}

#[test]
fn the_service_names_five_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 10);
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len());
}
