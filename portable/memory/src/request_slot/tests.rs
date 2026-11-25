extern crate std;

use core::sync::atomic::Ordering;
use std::{boxed::Box, sync::Arc, thread};

use super::*;

fn in_place<T>() -> &'static RequestSlot<T> {
    Box::leak(Box::new(RequestSlot::new_in_place()))
}

#[derive(Debug, PartialEq)]
struct Exchange {
    request: u32,
    answer: Option<u32>,
}

fn exchange(request: u32) -> Exchange {
    Exchange {
        request,
        answer: None,
    }
}

#[test]
fn in_place_service_borrows_the_same_place_and_reuses_it_after_take() {
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(client.is_idle());
    assert!(!server.service(|_| panic!("idle slot has no request")));
    assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    let mut address = None;
    for request in [3, 7] {
        client.post(exchange(request)).unwrap();
        assert!(!client.is_idle());
        assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
        assert!(server.service(|value| {
            let place = core::ptr::from_ref(value).addr();
            if let Some(previous) = address {
                assert_eq!(place, previous);
            }
            address = Some(place);
            assert_eq!(value.request, request);
            value.answer = Some(request * 2);
            assert_eq!(client.post(exchange(99)), Err(exchange(99)));
            assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
        }));
        assert!(!server.service(|_| panic!("answered request cannot execute twice")));
        assert_eq!(client.post(exchange(99)), Err(exchange(99)));
        let answer = client.try_take_if(|value| {
            assert_eq!(Some(core::ptr::from_ref(value).addr()), address);
            Ok::<_, ()>(())
        });
        assert_eq!(
            answer,
            Some(Ok(Exchange {
                request,
                answer: Some(request * 2)
            }))
        );
        assert!(client.is_idle());
        assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    }
    assert!(client.reunite(server).is_ok());
    assert!(slot.claim_in_place().is_some());
}

#[test]
fn in_place_validation_refusal_retains_the_exchange_until_a_valid_take() {
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    client.post(exchange(4)).unwrap();
    assert!(server.service(|value| value.answer = Some(8)));
    for _ in 0..2 {
        assert_eq!(
            client.try_take_if(|value| {
                assert_eq!(value.answer, Some(8));
                Err("foreign reply")
            }),
            Some(Err("foreign reply"))
        );
        assert_eq!(client.post(exchange(5)), Err(exchange(5)));
        assert!(!client.is_idle());
        assert!(!server.service(|_| panic!("a rejected reply stays answered")));
    }
    assert_eq!(
        client.try_take_if(|_| Ok::<_, ()>(())),
        Some(Ok(Exchange {
            request: 4,
            answer: Some(8)
        }))
    );
    assert!(client.reunite(server).is_ok());
}

#[derive(Debug)]
struct Retained(Arc<core::sync::atomic::AtomicUsize>);
impl Drop for Retained {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn in_place_abandonment_retains_the_payload_in_each_phase() {
    for phase in 0..3 {
        let drops = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let slot = in_place();
        let (mut client, mut server) = slot.claim_in_place().unwrap();
        client.post(Retained(drops.clone())).unwrap();
        match phase {
            0 => {
                drop(client);
                assert!(server.service(|_| {}));
            }
            1 => assert!(server.service(|_| drop(client))),
            _ => {
                assert!(server.service(|_| {}));
                drop(client);
            }
        }
        assert!(slot.claim_in_place().is_none());
        assert!(!server.service(|_| panic!("abandonment retains the answered seat")));
        drop(server);
        assert!(slot.claim_in_place().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn in_place_take_returns_the_owned_payload_exactly_once() {
    let drops = Arc::new(core::sync::atomic::AtomicUsize::new(0));
    let slot = in_place();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    client.post(Retained(drops.clone())).unwrap();
    assert!(server.service(|_| {}));
    let retained = client.try_take_if(|_| Ok::<_, ()>(())).unwrap().unwrap();
    assert!(client.try_take_if(|_| Ok::<_, ()>(())).is_none());
    assert!(client.reunite(server).is_ok());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(retained);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn in_place_reunion_refuses_occupied_or_foreign_endpoints_and_returns_both() {
    let slot = in_place();
    let other = in_place();
    let (mut client, server) = slot.claim_in_place().unwrap();
    let (other_client, other_server) = other.claim_in_place().unwrap();
    let (returned, other_server) = client.reunite(other_server).err().unwrap();
    client = returned;
    assert!(slot.claim_in_place().is_none());
    assert!(other.claim_in_place().is_none());
    assert!(other_client.reunite(other_server).is_ok());
    client.post(exchange(1)).unwrap();
    let (client, mut server) = client.reunite(server).err().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(server.service(|value| value.answer = Some(2)));
    let (mut client, server) = client.reunite(server).err().unwrap();
    assert!(slot.claim_in_place().is_none());
    assert!(matches!(
        client.try_take_if(|_| Ok::<_, ()>(())),
        Some(Ok(_))
    ));
    assert!(client.reunite(server).is_ok());
    assert!(slot.claim_in_place().is_some());
}

#[test]
fn in_place_post_service_and_take_race_without_loss_or_duplication() {
    const ROUNDS: u32 = 20_000;
    let slot = in_place::<Exchange>();
    let (mut client, mut server) = slot.claim_in_place().unwrap();
    let worker = thread::spawn(move || {
        for expected in 0..ROUNDS {
            while !server.service(|value| {
                assert_eq!(value.request, expected);
                value.answer = Some(expected * 3);
            }) {
                thread::yield_now();
            }
        }
        server
    });
    for request in 0..ROUNDS {
        client.post(exchange(request)).unwrap();
        loop {
            if let Some(answer) = client.try_take_if(|_| Ok::<_, ()>(())) {
                assert_eq!(
                    answer,
                    Ok(Exchange {
                        request,
                        answer: Some(request * 3)
                    })
                );
                break;
            }
            thread::yield_now();
        }
    }
    assert!(client.reunite(worker.join().unwrap()).is_ok());
}
