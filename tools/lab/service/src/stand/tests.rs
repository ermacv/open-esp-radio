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
fn only_a_router_operation_needs_the_air() {
    use oer_lab_client::protocol::RouterOperation;
    let board = || "02:00:00:00:00:01".to_owned();
    let alone = [Claim::board(&board()), Claim::shared(ROUTER)];
    let router = Operation::Router {
        request: RouterOperation::VerifyRuntime,
    };
    assert!(air_refusal(&router, &alone).is_some());
    assert!(air_refusal(&router, &[Claim::shared(ROUTER), Claim::shared(AIR)]).is_none());
    // A board's radio works only on its image's command.
    for operation in [
        Operation::Flash {
            board: board(),
            bundle: "bundle".to_owned(),
            image: oer_lab_arbiter::journal::ImageIdentity {
                name: "aes".to_owned(),
                sha256: String::new(),
                commit: None,
                dirty: None,
                origin: "test".to_owned(),
            },
            owner: "ann".to_owned(),
            via: FlashVia::Usb,
            skip_carried: false,
        },
        Operation::Reset {
            board: board(),
            path: None,
        },
        Operation::Console {
            board: board(),
            reset: true,
        },
        Operation::PowerCycle { board: board() },
    ] {
        assert!(air_refusal(&operation, &alone).is_none(), "{operation:?}");
    }
}
