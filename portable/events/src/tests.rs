use crate::{EventSink, report};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Dropped,
    Released(u8),
}

#[derive(Default)]
struct Kept {
    dropped: u32,
    released: u32,
}

impl EventSink<Event> for Kept {
    fn event(&mut self, event: Event) {
        match event {
            Event::Dropped => self.dropped = self.dropped.saturating_add(1),
            Event::Released(count) => {
                self.released = self.released.saturating_add(u32::from(count))
            }
        }
    }
}

fn owner(sink: &mut impl EventSink<Event>, built: &mut u32) {
    report(sink, || {
        *built += 1;
        Event::Dropped
    });
    report(sink, || {
        *built += 1;
        Event::Released(3)
    });
}

#[test]
fn the_empty_sink_never_builds_an_event() {
    let mut built = 0;
    owner(&mut (), &mut built);
    assert_eq!(built, 0);
    const { assert!(!<() as EventSink<Event>>::ENABLED) };
}

#[test]
fn an_enabled_sink_receives_every_event_once() {
    let mut kept = Kept::default();
    let mut built = 0;
    owner(&mut kept, &mut built);
    assert_eq!(built, 2);
    assert_eq!((kept.dropped, kept.released), (1, 3));
}

#[test]
fn a_lent_loan_forwards_to_the_callers_sink() {
    let mut kept = Kept::default();
    let mut built = 0;
    {
        let mut loan = &mut kept;
        owner(&mut loan, &mut built);
        const { assert!(<&mut Kept as EventSink<Event>>::ENABLED) };
    }
    let mut empty = ();
    let mut loan = &mut empty;
    owner(&mut loan, &mut built);
    assert_eq!(built, 2);
    assert_eq!((kept.dropped, kept.released), (1, 3));
}

#[test]
fn an_event_may_carry_a_key_borrowed_from_the_caller() {
    #[derive(Clone, Copy)]
    struct Keyed<'a>(&'a str);
    struct Last<'a>(Option<&'a str>);
    impl<'a> EventSink<Keyed<'a>> for Last<'a> {
        fn event(&mut self, event: Keyed<'a>) {
            self.0 = Some(event.0);
        }
    }
    let key = std::string::String::from("peer");
    report(&mut (), || Keyed(&key));
    let mut last = Last(None);
    report(&mut last, || Keyed(&key));
    assert_eq!(last.0, Some("peer"));
}
