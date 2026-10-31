use super::*;

#[test]
fn operations_on_one_board_take_turns_in_the_order_they_asked() {
    let owner = std::sync::Arc::new(BoardOwner::default());
    let never = AtomicBool::new(false);
    let first = owner.take(&never).unwrap();
    // Another board is not held up.
    drop(
        std::sync::Arc::new(BoardOwner::default())
            .take(&never)
            .unwrap(),
    );
    let order = std::sync::Arc::new(Mutex::new(Vec::new()));
    let waiters = (1..=2)
        .map(|waiter| {
            let (asking, order) = (owner.clone(), order.clone());
            let waiting = std::thread::spawn(move || {
                let _turn = asking.take(&AtomicBool::new(false)).unwrap();
                lock(&order).push(waiter);
            });
            // The second asks only once the first waits in the queue.
            while lock(&owner.queue).1.len() != waiter + 1 {
                std::thread::yield_now();
            }
            waiting
        })
        .collect::<Vec<_>>();
    assert!(lock(&order).is_empty());
    drop(first);
    for waiting in waiters {
        waiting.join().unwrap();
    }
    assert_eq!(*lock(&order), vec![1, 2]);
}

#[test]
fn a_stopped_operation_leaves_the_queue_without_its_turn() {
    let owner = std::sync::Arc::new(BoardOwner::default());
    let _first = owner.take(&AtomicBool::new(false)).unwrap();
    let refused = owner.take(&AtomicBool::new(true)).err().unwrap();
    assert!(refused.contains("stopped"), "{refused}");
    assert_eq!(lock(&owner.queue).1.len(), 1);
}

#[test]
fn what_may_radiate_needs_the_air() {
    use oer_lab_model::rom::ResetPath;
    let board = || "02:00:00:00:00:01".to_owned();
    let alone = [Claim::board(&board())];
    let with_air = [Claim::board(&board()), Claim::shared(AIR)];
    let starting = [
        Operation::Reset {
            board: board(),
            path: None,
        },
        Operation::Reset {
            board: board(),
            path: Some(ResetPath::Rts),
        },
        Operation::Console {
            board: board(),
            reset: true,
        },
        Operation::Reset {
            board: board(),
            path: Some(ResetPath::Download),
        },
        Operation::PowerCycle { board: board() },
        Operation::HubPort {
            hub: "top".to_owned(),
            port: 1,
            off_secs: 1,
        },
    ];
    for operation in &starting {
        assert!(air_refusal(operation, &alone).is_some(), "{operation:?}");
        assert!(air_refusal(operation, &with_air).is_none(), "{operation:?}");
    }
    let quiet = [
        Operation::ReadConsole {
            board: board(),
            watch_ms: 1,
        },
        Operation::Console {
            board: board(),
            reset: false,
        },
        Operation::Describe { board: board() },
    ];
    for operation in &quiet {
        assert!(air_refusal(operation, &alone).is_none(), "{operation:?}");
    }
}
