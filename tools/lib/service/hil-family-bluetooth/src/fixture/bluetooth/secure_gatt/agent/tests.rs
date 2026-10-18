use super::*;

fn message(sender: &str) -> zbus::Message {
    zbus::Message::method_call("/agent", "RequestConfirmation")
        .unwrap()
        .sender(sender)
        .unwrap()
        .build(&())
        .unwrap()
}
fn agent() -> (Agent, async_channel::Receiver<Prompt>) {
    let (prompts, rx) = async_channel::bounded(1);
    (
        Agent {
            sender: ":1.42".into(),
            device: "/dut".into(),
            prompts,
            pending: Arc::new(Mutex::new(None)),
        },
        rx,
    )
}
#[test]
fn callback_waits_for_explicit_response_and_dropped_prompt_declines() {
    for accept in [false, true] {
        let (agent, rx) = agent();
        let message = message(":1.42");
        let (result, ()) = futures_lite::future::block_on(futures_lite::future::zip(
            agent.request_confirmation(
                OwnedObjectPath::try_from("/dut").unwrap(),
                123,
                message.header(),
            ),
            async {
                let prompt = rx.recv().await.unwrap();
                assert_eq!(prompt.number, 123);
                if accept {
                    prompt.answer(true).unwrap();
                } else {
                    drop(prompt);
                }
            },
        ));
        assert_eq!(result.is_ok(), accept);
        assert!(agent.pending.lock().unwrap().is_none());
    }
}
#[test]
fn callback_rejects_other_sender_device_and_non_numeric_methods() {
    let (agent, _rx) = agent();
    for (sender, device, number) in [
        (":1.43", "/dut", 123),
        (":1.42", "/other", 123),
        (":1.42", "/dut", 1_000_000),
    ] {
        let message = message(sender);
        assert!(
            futures_lite::future::block_on(agent.request_confirmation(
                OwnedObjectPath::try_from(device).unwrap(),
                number,
                message.header()
            ))
            .is_err()
        );
    }
    assert!(
        agent
            .request_authorization(OwnedObjectPath::try_from("/dut").unwrap())
            .is_err()
    );
    assert!(
        agent
            .request_passkey(OwnedObjectPath::try_from("/dut").unwrap())
            .is_err()
    );
}
#[test]
fn cancellation_cannot_free_the_slot_before_old_callback_finishes() {
    let (agent, rx) = agent();
    let message = message(":1.42");
    let (result, ()) = futures_lite::future::block_on(futures_lite::future::zip(
        agent.request_confirmation(
            OwnedObjectPath::try_from("/dut").unwrap(),
            123,
            message.header(),
        ),
        async {
            let prompt = rx.recv().await.unwrap();
            agent.cancel(message.header()).unwrap();
            assert!(
                agent
                    .request_confirmation(
                        OwnedObjectPath::try_from("/dut").unwrap(),
                        123,
                        message.header()
                    )
                    .await
                    .is_err()
            );
            assert!(prompt.answer(true).is_err());
        },
    ));
    assert!(result.is_err());
    assert!(agent.pending.lock().unwrap().is_none());
}
