//! Independent CCMP known-answer tests; only vector facts are incorporated.
//!
//! SOURCE: IEEE Std 802.11-2020 Annex J.6.4 (CCMP-128 PV0) and J.9.2
//! (individually addressed Deauthentication). These are also carried by
//! hostap's `wlantest/test_vectors.c` as 802.11-2012 M.6.4 and M.9.2.
//!
//! SOURCE: BSD-3-Clause hostap `d945ddd368085f255e68328f2d3b020ceea359af`
//! (hostap_2_11), `wlantest/ccmp.c` and `src/crypto/aes-ccm.c`.
//! https://w1.fi/cgit/hostap/tree/wlantest/ccmp.c?id=d945ddd368085f255e68328f2d3b020ceea359af
//! QoS/4-address outputs were measured with that unmodified wlantest
//! transform and OpenSSL AES-128. Inputs extend J.6.4 with TID 5, fragment
//! 5, ToDS/FromDS and A4 02:00:00:00:00:04; the four-address QoS input also
//! sets A-MSDU-present, masked in ordinary CCMP. These are generated peer
//! vectors, not additional published Annex J vectors. No C code is ported.

use oer_ieee80211_cipher::ccmp_mpdu::{CcmpAssociation, CcmpMpduError};
use oer_ieee80211_wire::{ccmp::Ccmp128Key, security::Spp};

struct Vector {
    key: &'static str,
    header: &'static str,
    body: &'static str,
    wire: &'static str,
}

const ANNEX_J_6_4: Vector = Vector {
    key: "c97c1f67ce371185514a8a19f2bdd52f",
    header: "0848c32c0fd2e128a57c5030f1844408abaea5b8fcba8033",
    body: "f8ba1a55d02f85ae967bb62fb6cda8eb7e78a050",
    wire: "0848c32c0fd2e128a57c5030f1844408abaea5b8fcba80330ce70020769703b5f3d0a2fe9a3dbf2342a643e43246e80c3c04d0197845ce0b16f97623",
};

const QOS: Vector = Vector {
    key: "c97c1f67ce371185514a8a19f2bdd52f",
    header: "8849c32c0fd2e128a57c5030f1844408abaea5b8fcba85330500",
    body: "f8ba1a55d02f85ae967bb62fb6cda8eb7e78a050",
    wire: "8849c32c0fd2e128a57c5030f1844408abaea5b8fcba853305000ce70020769703b52a9fcd87ea9d63ab12f683107947fe820ff7500847aa5ca5e6a6b8f9",
};

const FOUR_ADDRESS: Vector = Vector {
    key: "c97c1f67ce371185514a8a19f2bdd52f",
    header: "084bc32c0fd2e128a57c5030f1844408abaea5b8fcba8533020000000004",
    body: "f8ba1a55d02f85ae967bb62fb6cda8eb7e78a050",
    wire: "084bc32c0fd2e128a57c5030f1844408abaea5b8fcba85330200000000040ce70020769703b5f3d0a2fe9a3dbf2342a643e43246e80c3c04d019cd993e3b2cf7bd3b",
};

const FOUR_ADDRESS_QOS: Vector = Vector {
    key: "c97c1f67ce371185514a8a19f2bdd52f",
    header: "884bc32c0fd2e128a57c5030f1844408abaea5b8fcba85330200000000048500",
    body: "f8ba1a55d02f85ae967bb62fb6cda8eb7e78a050",
    wire: "884bc32c0fd2e128a57c5030f1844408abaea5b8fcba853302000000000485000ce70020769703b52a9fcd87ea9d63ab12f683107947fe820ff750086c288642049b16c3",
};

const ANNEX_J_9_2: Vector = Vector {
    key: "66ed21042f9f26d7115706e40414cf2e",
    header: "c04000000200000001000200000000000200000000006000",
    body: "0200",
    wire: "c0400000020000000100020000000000020000000000600001000020000000001d07cafd0409bb8bafef",
};

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn assert_vector(vector: &Vector) {
    let key = Ccmp128Key::new(bytes(vector.key).as_slice().try_into().unwrap());
    let association = CcmpAssociation::new(Spp::NotNegotiated);
    let wire = bytes(vector.wire);
    let header = bytes(vector.header);
    let body = bytes(vector.body);
    let mut opened = wire.clone();
    assert_eq!(association.open(&key, &mut opened).unwrap(), body);
    assert_eq!(&opened[..header.len()], header);

    // Encrypt from the independently specified plaintext, not a round trip.
    let mut plain = header.clone();
    plain.extend_from_slice(&wire[header.len()..header.len() + 8]);
    plain.extend_from_slice(&body);
    plain.extend_from_slice(&[0; 8]);
    association.seal(&key, &mut plain).unwrap();
    assert_eq!(plain, wire);

    let mut damaged = wire.clone();
    *damaged.last_mut().unwrap() ^= 1;
    assert_eq!(
        association.open(&key, &mut damaged),
        Err(CcmpMpduError::InvalidMic)
    );
    assert!(
        damaged[header.len() + 8..damaged.len() - 8]
            .iter()
            .all(|byte| *byte == 0)
    );
}

#[test]
fn ieee_80211_2020_annex_j_6_4_ccmp_known_answer() {
    assert_vector(&ANNEX_J_6_4);
}

#[test]
fn ieee_80211_2020_annex_j_9_2_management_known_answer() {
    assert_vector(&ANNEX_J_9_2);
}

#[test]
fn hostap_wlantest_qos_and_four_address_known_answers() {
    for vector in [&QOS, &FOUR_ADDRESS, &FOUR_ADDRESS_QOS] {
        assert_vector(vector);
        let key = Ccmp128Key::new(bytes(vector.key).as_slice().try_into().unwrap());
        let mut wire = bytes(vector.wire);
        // Bind A4 in 28/30-byte AAD, or the TID in 24-byte AAD.
        let aad_offset = if vector.header.len() / 2 >= 30 {
            29
        } else {
            24
        };
        wire[aad_offset] ^= 1;
        assert_eq!(
            CcmpAssociation::new(Spp::NotNegotiated).open(&key, &mut wire),
            Err(CcmpMpduError::InvalidMic)
        );
    }
}
