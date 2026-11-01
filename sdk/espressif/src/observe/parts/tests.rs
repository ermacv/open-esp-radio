use super::*;

#[test]
fn a_part_counts_its_polls_and_their_time_with_the_open_one() {
    let counter = PartCounter::new();
    assert_eq!(
        counter.sample(5, 0),
        Ok(PartSample::default()),
        "nothing seen"
    );
    counter.observe(10, false, 0);
    counter.observe(25, true, 0);
    counter.observe(30, false, 0);
    counter.observe(70, true, 0);
    counter.observe(100, false, 0);
    assert_eq!(
        counter.sample(104, 0),
        Ok(PartSample {
            polls: 2,
            poll_us: 15 + 40 + 4,
            ended_us: 15 + 40,
            longest_us: Some(40),
        })
    );
}

#[test]
fn the_longest_poll_counts_only_in_its_epoch() {
    let counter = PartCounter::new();
    counter.observe(0, false, 0);
    counter.observe(500, true, 0);
    assert_eq!(counter.sample(600, 1).unwrap().longest_us, None);
    counter.observe(600, false, 1);
    counter.observe(620, true, 1);
    counter.observe(700, false, 1);
    counter.observe(710, true, 1);
    assert_eq!(counter.sample(800, 1).unwrap().longest_us, Some(20));
}

#[test]
fn an_end_without_its_start_adds_nothing_and_spans_cross_the_wrap() {
    let counter = PartCounter::new();
    counter.observe(10, true, 0);
    assert_eq!(counter.sample(20, 0), Ok(PartSample::default()));
    counter.observe(u32::MAX - 4, false, 0);
    counter.observe(5, true, 0);
    assert_eq!(counter.sample(6, 0).unwrap().poll_us, 10);
}

#[test]
fn a_sample_during_a_write_is_refused() {
    let counter = PartCounter::new();
    counter.sequence.store(1, Ordering::Relaxed);
    assert_eq!(counter.sample(0, 0), Err(Concurrent));
}

#[test]
fn library_parts_have_distinct_names() {
    for (index, part) in LibraryPart::ALL.iter().enumerate() {
        assert_eq!(*part as usize, index);
        assert!(
            LibraryPart::ALL[..index]
                .iter()
                .all(|earlier| earlier.name() != part.name()),
            "{} is named once",
            part.name()
        );
    }
}

#[test]
fn an_image_names_each_of_its_parts_once_after_the_library_s() {
    let names = ImageNames::new();
    assert_eq!(names.name(0), Some("application"));
    assert_eq!(names.name(LIBRARY_PARTS), None, "not named yet");
    let numbers: std::vec::Vec<_> = ["agent", "relay", "load", "spare"]
        .into_iter()
        .map(|name| names.claim(name))
        .collect();
    assert_eq!(
        numbers,
        (LIBRARY_PARTS..PARTS)
            .map(Some)
            .collect::<std::vec::Vec<_>>()
    );
    assert_eq!(names.claim("more"), None, "every image part is named");
    assert_eq!(names.name(LIBRARY_PARTS + 2), Some("load"));
    assert_eq!(names.name(PARTS), None);
}
