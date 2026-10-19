extern crate std;
use super::*;
use std::{
    sync::{Arc, mpsc},
    thread,
};

// A generative two-place test mint. No public constructor or Clone; the
// tests recycle the very same permission rather than manufacturing a credit.
#[derive(Debug)]
struct Permission<'id> {
    index: usize,
    generation: usize,
    drops: Arc<AtomicUsize>,
    _brand: PhantomData<fn(&'id ()) -> &'id ()>,
}
fn mint(drops: Arc<AtomicUsize>, body: impl for<'id> FnOnce([Permission<'id>; 2])) {
    body(core::array::from_fn(|index| Permission {
        index,
        generation: 0,
        drops: drops.clone(),
        _brand: PhantomData,
    }));
}
impl Drop for Permission<'_> {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
#[allow(
    unsafe_code,
    reason = "the test mint issues two distinct invariant branded indices"
)]
// SAFETY: only mint creates these two owners in a fresh invariant domain;
// neither is cloneable. Reuse moves the same received owner after a channel
// transfer or on the same thread, so prior reads happen before the next write.
unsafe impl UniqueQueueIndex<2> for Permission<'_> {
    fn queue_index(&self) -> usize {
        self.index
    }
}

#[test]
fn all_places_fit_and_reuse_waits_for_the_owned_response() {
    let drops = Arc::new(AtomicUsize::new(0));
    mint(drops.clone(), |[a, b]| {
        let mut queue = AffineIndexedSpscQueue::<Permission<'_>, 2>::new();
        let (mut tx, mut rx) = queue.split();
        tx.send(a);
        tx.send(b);
        let mut reply = rx.try_receive().unwrap();
        assert_eq!(reply.index, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        // The first value is retained in a reply while the second is received;
        // no free-credit lookup is possible or needed at either endpoint.
        let second = rx.try_receive().unwrap();
        assert!(matches!(
            rx.try_receive(),
            Err(AffineSpscTryReceiveError::Empty)
        ));
        reply.generation += 1;
        tx.send(reply);
        tx.send(second);
        for generation in 1..128 {
            let mut a = rx.try_receive().unwrap();
            let b = rx.try_receive().unwrap();
            assert_eq!((a.index, a.generation, b.index), (0, generation, 1));
            a.generation += 1;
            tx.send(a);
            tx.send(b);
        }
        // Dropping the store destroys both remaining initialized values once.
    });
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn cross_core_send_receive_and_response_interleavings_preserve_cell_ownership() {
    for return_before_second_receive in [false, true] {
        let drops = Arc::new(AtomicUsize::new(0));
        mint(drops.clone(), |[a, b]| {
            let mut queue = AffineIndexedSpscQueue::<Permission<'_>, 2>::new();
            let (mut tx, mut rx) = queue.split();
            let (published, publication) = mpsc::channel();
            let (respond, responses) = mpsc::channel();
            thread::scope(|threads| {
                threads.spawn(move || {
                    for generation in 0..128 {
                        publication.recv().unwrap();
                        let a = rx.try_receive().unwrap();
                        assert_eq!((a.index, a.generation), (0, generation));
                        if return_before_second_receive {
                            respond.send(a).unwrap();
                            publication.recv().unwrap();
                            let b = rx.try_receive().unwrap();
                            assert_eq!((b.index, b.generation), (1, generation));
                            respond.send(b).unwrap();
                        } else {
                            publication.recv().unwrap();
                            let b = rx.try_receive().unwrap();
                            assert_eq!((b.index, b.generation), (1, generation));
                            respond.send(a).unwrap();
                            respond.send(b).unwrap();
                        }
                    }
                    assert!(matches!(
                        rx.try_receive(),
                        Err(AffineSpscTryReceiveError::Empty)
                    ));
                });
                tx.send(a);
                published.send(()).unwrap();
                tx.send(b);
                published.send(()).unwrap();
                for _ in 1..128 {
                    let mut a = responses.recv().unwrap();
                    a.generation += 1;
                    tx.send(a);
                    published.send(()).unwrap();
                    let mut b = responses.recv().unwrap();
                    b.generation += 1;
                    tx.send(b);
                    published.send(()).unwrap();
                }
                drop(responses.recv().unwrap());
                drop(responses.recv().unwrap());
            });
        });
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }
}
