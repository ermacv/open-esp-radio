extern crate std;
use super::*;
use core::cell::RefCell;
use std::{panic::AssertUnwindSafe, rc::Rc, vec, vec::Vec};

struct Tracked {
    id: u8,
    log: Rc<RefCell<Vec<u8>>>,
}
impl Tracked {
    fn new(id: u8, log: &Rc<RefCell<Vec<u8>>>) -> Self {
        Self {
            id,
            log: log.clone(),
        }
    }
}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.log.borrow_mut().push(self.id);
    }
}
struct Pair<A, B> {
    first: A,
    second: B,
}

#[test]
fn projection_and_transfer_retain_one_destruction_right_per_field() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut source = MaybeUninit::<Pair<Tracked, Tracked>>::uninit();
    let mut destination = MaybeUninit::uninit();
    let pair = Placed::init(&mut source, |p| {
        crate::init_in_place!(
            p,
            Pair {
                first: Tracked::new(1, &log),
                second: Tracked::new(2, &log),
            }
        )
    });
    let (first, second) = crate::unpack_placed!(pair, Pair { first, second });
    let first = first.move_to(&mut destination);
    assert!(log.borrow().is_empty());
    drop(second.read());
    assert_eq!(*log.borrow(), vec![2]);
    drop(first);
    assert_eq!(*log.borrow(), vec![2, 1]);
}

#[test]
fn replacement_transfers_the_owner_before_dropping_the_previous_destination() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut storage = MaybeUninit::uninit();
    let source = Placed::init(&mut storage, |p| p.write(Tracked::new(1, &log)));
    let mut destination = Tracked::new(2, &log);
    source.replace_into(&mut destination);
    assert_eq!(destination.id, 1);
    assert_eq!(*log.borrow(), vec![2]);
    drop(destination);
    assert_eq!(*log.borrow(), vec![2, 1]);
}

#[test]
fn an_unwinding_old_destructor_leaves_the_new_destination_owned() {
    struct Owner {
        fail: bool,
        count: Rc<core::cell::Cell<usize>>,
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            self.count.set(self.count.get() + 1);
            assert!(!self.fail, "old destructor unwinds");
        }
    }
    let count = Rc::new(core::cell::Cell::new(0));
    let mut storage = MaybeUninit::uninit();
    let source = Placed::init(&mut storage, |p| {
        p.write(Owner {
            fail: false,
            count: count.clone(),
        })
    });
    let mut destination = Owner {
        fail: true,
        count: count.clone(),
    };
    let result =
        std::panic::catch_unwind(AssertUnwindSafe(|| source.replace_into(&mut destination)));
    assert!(result.is_err());
    assert!(!destination.fail);
    assert_eq!(count.get(), 1);
    drop(destination);
    assert_eq!(count.get(), 2);
}

#[test]
fn exhaustive_enum_projection_drops_unselected_owners_once() {
    enum Outcome {
        Stopped(Tracked),
        Failed { owner: Tracked, retained: Tracked },
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    for failed in [false, true] {
        let mut storage = MaybeUninit::uninit();
        let value = Placed::init(&mut storage, |p| {
            p.write(if failed {
                Outcome::Failed {
                    owner: Tracked::new(3, &log),
                    retained: Tracked::new(4, &log),
                }
            } else {
                Outcome::Stopped(Tracked::new(1, &log))
            })
        });
        let owner = crate::match_placed!(value, Outcome {
            Stopped(owner) => owner.read(),
            Failed { owner, retained } => { drop(retained); owner.read() },
        });
        drop(owner);
    }
    assert_eq!(*log.borrow(), vec![1, 4, 3]);
}

#[test]
fn array_initialization_cleans_only_its_completed_prefix_on_panic() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut storage = MaybeUninit::<[Tracked; 4]>::uninit();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        init_array(&mut storage, |index| {
            if index == 2 {
                panic!("fixture interrupts initialization");
            }
            Tracked::new(index as u8, &log)
        });
    }));
    assert!(result.is_err());
    assert_eq!(*log.borrow(), vec![0, 1]);
}

#[test]
fn a_large_future_output_is_handled_through_its_loan_and_retires_once() {
    struct Output {
        bytes: [u8; 8192],
        resource: Tracked,
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut polls = 0;
    let mut future = core::pin::pin!(core::future::poll_fn(|_| {
        polls += 1;
        if polls == 1 {
            Poll::Pending
        } else {
            Poll::Ready(Output {
                bytes: [0x53; 8192],
                resource: Tracked::new(5, &log),
            })
        }
    }));
    let mut storage = MaybeUninit::uninit();
    let mut placed = core::pin::pin!(poll_placed(future.as_mut(), &mut storage));
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(placed.as_mut().poll(&mut cx).is_pending());
    assert!(log.borrow().is_empty());
    let Poll::Ready(output) = placed.as_mut().poll(&mut cx) else {
        panic!("second poll completes");
    };
    let (bytes, resource) = crate::unpack_placed!(output, Output { bytes, resource });
    assert!(bytes.iter().all(|b| *b == 0x53));
    drop(bytes);
    assert!(log.borrow().is_empty());
    drop(resource);
    assert_eq!(*log.borrow(), vec![5]);
}
