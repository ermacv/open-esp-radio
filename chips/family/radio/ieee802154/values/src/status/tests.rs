// SPDX-License-Identifier: Apache-2.0
use core::cmp::Ordering;

use super::{Ieee802154RxStateCode, Ieee802154StateSnapshot, Ieee802154TxStateCode};

#[test]
fn receive_predicates_use_the_chip_classification_and_keep_the_diagnostic_image() {
    // A fictitious chip can assign RECEIVE_SFD any image. The shared values
    // retain the adapter's classification without knowing a hardware code.
    for (relation, at_sfd, after_sfd) in [
        (Ordering::Less, false, false),
        (Ordering::Equal, true, false),
        (Ordering::Greater, false, true),
    ] {
        let state = Ieee802154RxStateCode::from_parts(200, relation);
        assert_eq!(state.is_receive_sfd(), at_sfd);
        assert_eq!(state.is_after_receive_sfd(), after_sfd);
        assert_eq!(state.value(), 200);
    }
}

#[test]
fn zero_observations_test_only_the_numeric_images() {
    for (rx, tx, all_zero) in [(0, 0, true), (0, 1, false), (1, 0, false), (1, 1, false)] {
        let state = Ieee802154StateSnapshot::new(
            Ieee802154RxStateCode::from_parts(rx, Ordering::Greater),
            Ieee802154TxStateCode::from_field(tx),
        );
        assert_eq!(state.all_codes_zero(), all_zero);
        assert_eq!(state.rx().value(), rx);
        assert_eq!(state.tx().value(), tx);
    }
}
