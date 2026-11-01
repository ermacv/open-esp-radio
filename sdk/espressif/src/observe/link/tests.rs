use super::*;

#[test]
fn an_aggregate_counts_in_the_bin_of_its_subframes() {
    let bins: std::vec::Vec<_> = [1, 2, 3, 4, 5, 8, 9, 16, 17, 32]
        .into_iter()
        .map(size_bin)
        .collect();
    assert_eq!(bins, [0, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
}

#[test]
fn settlements_and_block_ack_requests_count_apart() {
    let counter = LinkCounter::new();
    counter.settled(Settled {
        members: 5,
        refused: false,
        acknowledged: 4,
        retried: 1,
        dropped: 1,
    });
    counter.settled(Settled {
        members: 1,
        refused: true,
        ..Settled::default()
    });
    counter.block_ack_request();
    assert_eq!(
        counter.ampdu(),
        TxAmpdu {
            aggregates: 2,
            refused: 1,
            subframes: 6,
            by_size: [1, 0, 0, 1, 0, 0],
            acknowledged: 4,
            retried: 1,
            dropped: 1,
            block_ack_requests: 1,
        }
    );
}

#[test]
fn each_rate_counts_in_its_own_bucket() {
    let counter = LinkCounter::new();
    let ht = |mcs, width40, short_gi| Rate::Ht {
        mcs,
        width40,
        short_gi,
    };
    let receive = |rate, times| {
        for _ in 0..times {
            counter.received(rate, Containment::InAmpdu);
        }
    };
    receive(ht(7, true, true), 5);
    receive(ht(7, true, false), 4);
    receive(ht(0, false, false), 3);
    receive(ht(6, false, true), 2);
    receive(ht(8, true, true), 9);
    receive(ht(31, false, false), 1);
    receive(Rate::NonHt, 6);
    receive(Rate::He, 7);
    receive(Rate::Unknown, 8);
    let rx = counter.link().rx;
    let rates = rx.mpdus;
    assert_eq!(rates.ht40_short_gi, [0, 0, 0, 0, 0, 0, 0, 5]);
    assert_eq!(rates.ht40_long_gi, [0, 0, 0, 0, 0, 0, 0, 4]);
    assert_eq!(rates.ht20_long_gi, [3, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(rates.ht20_short_gi, [0, 0, 0, 0, 0, 0, 2, 0]);
    assert_eq!(
        (rates.ht_other, rates.non_ht, rates.he, rates.unknown),
        (10, 6, 7, 8)
    );
    assert_eq!((rx.in_ampdu, rx.single, rx.containment_unknown), (45, 0, 0));
    let distinct: std::collections::BTreeSet<_> = (0..=7u8)
        .flat_map(|mcs| {
            [(false, false), (false, true), (true, false), (true, true)]
                .map(|(width40, short_gi)| bucket(ht(mcs, width40, short_gi)))
        })
        .collect();
    assert_eq!(distinct, (0..HT_OTHER).collect());
}

#[test]
fn a_transmission_counts_its_rate_end_and_attempts() {
    let counter = LinkCounter::new();
    counter.sent(Rate::NonHt, TxEnd::Delivered, 1);
    counter.sent(Rate::NonHt, TxEnd::AckTimeout, 4);
    counter.sent(Rate::He, TxEnd::Unsent, 0);
    counter.received(Rate::Unknown, Containment::Single);
    counter.received(Rate::Unknown, Containment::Unknown);
    let link = counter.link();
    assert_eq!(
        (
            link.tx.mpdus.non_ht,
            link.tx.mpdus.he,
            link.tx.transmissions,
            link.tx.delivered,
            link.tx.ack_timeouts,
            link.tx.other_ends,
            link.tx.unsent
        ),
        (2, 1, 5, 1, 1, 0, 1)
    );
    assert_eq!((link.rx.single, link.rx.containment_unknown), (1, 1));
}
