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
