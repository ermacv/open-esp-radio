use super::*;

#[test]
fn only_a_download_boot_over_usb_parks_a_board() {
    assert!(serves_usb_download(
        "rst:0x1 (POWERON),boot:0x8 (DOWNLOAD(USB/UART0))"
    ));
    assert!(!serves_usb_download(
        "rst:0x15 (USB_UART_HPSYS),boot:0x0 (DOWNLOAD(UART0/SDIO_FEI_FEO))"
    ));
    assert!(!serves_usb_download(
        "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)"
    ));
    assert!(!serves_usb_download("DOWNLOAD(USB) without a boot field"));
}

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
fn parkings_of_two_boards_keep_both_records_of_their_power() {
    let directory = tempfile::tempdir().unwrap();
    let journal = oer_lab_arbiter::Arbiter::at(directory.path())
        .unwrap()
        .journal();
    let drivers = std::sync::Arc::new(StandDrivers::new(
        directory.path().to_owned(),
        directory.path().join("stand.toml"),
        directory.path().to_owned(),
        journal,
    ));
    let parkings = (0..8u8)
        .map(|board| {
            let drivers = drivers.clone();
            std::thread::spawn(move || {
                drivers
                    .powered_off(|records| {
                        records.insert(
                            format!("02:00:00:00:00:{board:02X}"),
                            HubPort {
                                location: "3-8".to_owned(),
                                port: board,
                            },
                        );
                        Ok(())
                    })
                    .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for parking in parkings {
        parking.join().unwrap();
    }
    assert_eq!(drivers.powered_off(|records| Ok(records.len())).unwrap(), 8);
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
        Operation::Reset {
            board: board(),
            path: Some(ResetPath::Download),
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

#[test]
fn a_board_parked_off_is_powered_only_for_a_lease_of_the_air() {
    let directory = tempfile::tempdir().unwrap();
    let journal = oer_lab_arbiter::Arbiter::at(directory.path())
        .unwrap()
        .journal();
    let drivers = StandDrivers::new(
        directory.path().to_owned(),
        directory.path().join("stand.toml"),
        directory.path().to_owned(),
        journal,
    );
    let mac = "02:00:00:00:00:01";
    drivers
        .powered_off(|records| {
            records.insert(
                mac.to_owned(),
                HubPort {
                    location: "3-8".to_owned(),
                    port: 1,
                },
            );
            Ok(())
        })
        .unwrap();
    let refused = drivers
        .prepare(&format!("board:{mac}"), &[Claim::board(mac)])
        .unwrap_err();
    assert!(refused.contains("must hold the air"), "{refused}");
    // A board that rests powered keeps its lease without the air.
    assert_eq!(
        drivers.prepare(
            "board:02:00:00:00:00:02",
            &[Claim::board("02:00:00:00:00:02")]
        ),
        Ok(())
    );
}
