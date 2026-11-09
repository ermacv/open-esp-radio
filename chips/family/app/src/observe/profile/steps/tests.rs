use super::*;

/// One publication attempt from cycle `at`: its choice of `choice` cycles
/// pulling `examined` works, then, for a chosen work, the port's end after
/// `transmit` cycles and the answer after `answer`.
fn attempt(
    tally: &mut ProfileTally,
    counter: &ProfileCounter,
    at: u64,
    [choice, transmit, answer]: [u64; 3],
    examined: u16,
    end: Option<Option<Unsent>>,
) -> u64 {
    tally.choice_began(at);
    let chose = at + choice;
    tally.chose(counter, chose, examined, 1, Some(end.is_some()));
    let Some(unsent) = end else {
        return chose;
    };
    let sent = chose + transmit;
    tally.transmitted(counter, sent, unsent);
    let answered = sent + answer;
    tally.answered(counter, answered);
    answered
}

/// The steps each count once, with their cycles, and each turn adds its
/// own sums of them: two turns' attempts, notices and flushes, one with a
/// port refusal and one choice that found nothing.
#[test]
fn each_step_counts_its_cycles_once_and_each_turn_its_sum() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    // Turn one: a notice phase of 300 cycles holding one debit of 120, two
    // attempts that went, the end of publishing, and a flush of 40.
    tally.began(&counter, 1_000);
    tally.notices_began(1_010);
    tally.notice_at(1_100);
    tally.settled(&counter, 1_220);
    tally.notices_ended(&counter, 1_310);
    let at = attempt(&mut tally, &counter, 1_400, [500, 2_000, 60], 3, Some(None));
    let at = attempt(&mut tally, &counter, at, [400, 1_000, 40], 2, Some(None));
    let at = attempt(&mut tally, &counter, at, [100, 0, 0], 0, None);
    tally.published(at);
    tally.flushed(&counter, at + 40);
    tally.ended(&counter, at + 50);
    // Turn two: an attempt the port refused, then publishing ended refused
    // with nothing flushed.
    tally.began(&counter, 20_000);
    tally.notices_began(20_000);
    tally.notices_ended(&counter, 20_010);
    let at = attempt(
        &mut tally,
        &counter,
        20_100,
        [300, 700, 30],
        1,
        Some(Some(Unsent::Port)),
    );
    tally.published(at);
    tally.ended(&counter, at + 10);
    let steps = counter.read_steps();
    let square = |values: &[u64]| values.iter().map(|value| value * value).sum::<u64>();
    let expect = |values: &[u64]| [values.len() as u64, values.iter().sum(), square(values)];
    assert_eq!(
        steps.steps[Step::Choice as usize],
        expect(&[500, 400, 100, 300])
    );
    assert_eq!(
        steps.steps[Step::Transmit as usize],
        expect(&[2_000, 1_000, 700])
    );
    assert_eq!(steps.steps[Step::Answer as usize], expect(&[60, 40, 30]));
    assert_eq!(steps.steps[Step::Flush as usize], expect(&[40]));
    assert_eq!(steps.steps[Step::Notices as usize], expect(&[300, 10]));
    assert_eq!(steps.steps[Step::Debit as usize], expect(&[120]));
    // By turn: the first turn's choices sum to 1000, the second's to 300.
    assert_eq!(
        steps.turn_steps[Step::Choice as usize],
        expect(&[1_000, 300])
    );
    assert_eq!(
        steps.turn_steps[Step::Transmit as usize],
        expect(&[3_000, 700])
    );
    assert_eq!(steps.turn_steps[Step::Flush as usize], expect(&[40]));
    assert_eq!(steps.turn_steps[Step::Debit as usize], expect(&[120]));
    assert_eq!(steps.weighed[..2], [6, 4], "works pulled, passes begun");
    assert_eq!(
        steps.chosen,
        [3, 1, 0, 0],
        "three works, one empty list, none withheld"
    );
    assert_eq!(steps.unsent[Unsent::Port as usize], 1);
    assert_eq!(steps.unsent.iter().sum::<u64>(), 1);
    assert_eq!(steps.examined_by_choice, [1, 1, 1, 1, 0, 0, 0]);
    assert_eq!(steps.accepted_by_flush[2], 1, "one flush, two accepted");
    assert_eq!(steps.accepted_by_flush.iter().sum::<u64>(), 1);
    assert_eq!(steps.choices_by_turn[3], 1, "turn one chose three times");
    assert_eq!(steps.choices_by_turn[1], 1, "turn two once");
    // The turn profile counts the accepted frames the steps saw.
    assert_eq!(counter.read().moment(0, 2), 2, "two accepted, in turn one");
}

/// A choice that pulled works and chose none is withheld work, apart from
/// one that found the list empty; what the policy weighed and why it
/// withheld add up as reported.
#[test]
fn a_choice_that_pulled_works_and_chose_none_withheld_them() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    tally.began(&counter, 0);
    tally.choice_began(10);
    tally.weighed(&counter, 4, 2, false);
    tally.withheld(&counter, Withheld::Capacity);
    tally.chose(&counter, 90, 4, 1, Some(false));
    tally.choice_began(100);
    tally.weighed(&counter, 15, 4, true);
    tally.chose(&counter, 130, 5, 3, None);
    tally.published(130);
    tally.ended(&counter, 140);
    let steps = counter.read_steps();
    assert_eq!(steps.chosen, [0, 0, 1, 1], "one withheld, one fault");
    assert_eq!(steps.weighed, [9, 4, 19, 6, 1]);
    assert_eq!(steps.withheld, [0, 1, 0]);
    assert_eq!(steps.examined_by_choice[3], 1, "four works");
    assert_eq!(steps.examined_by_choice[4], 1, "five works");
    assert_eq!(steps.steps[Step::Transmit as usize], [0, 0, 0], "none went");
}

/// Loans given back and blocks brought home count by turn; a turn with
/// none of the four items that only brought something home is a home-only
/// turn with its cycles, and an empty turn that brought nothing is not.
#[test]
fn home_only_turns_are_empty_turns_that_brought_something_home() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    tally.began(&counter, 0);
    tally.home(0, 3);
    tally.ended(&counter, 700);
    tally.began(&counter, 1_000);
    tally.ended(&counter, 1_300);
    tally.began(&counter, 2_000);
    tally.home(2, 1);
    tally.frame();
    tally.ended(&counter, 4_000);
    let steps = counter.read_steps();
    assert_eq!(
        steps.home,
        [4, 2, 2, 1],
        "loans, their turns, blocks, theirs"
    );
    assert_eq!(steps.home_only, [1, 700, 700 * 700, 3]);
    assert_eq!(
        steps.loans_by_turn,
        [1, 0, 1, 0, 0, 0],
        "one of 1, one of 3"
    );
    let profile = counter.read();
    assert_eq!(profile.empty[0], 2, "both item-less turns are empty");
}

/// Each take that left frames unlent for want of credit adds the loans then,
/// at once, whatever the turn does after.
#[test]
fn a_take_without_credit_adds_the_loans_out_and_back_then() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    tally.began(&counter, 0);
    tally.no_credit(&counter, 1, 12, 0);
    tally.no_credit(&counter, 2, 12, 3);
    tally.no_credit(&counter, 1, 11, 9);
    let steps = counter.read_steps();
    assert_eq!(steps.no_credit, [3, 4, 35, 12, 2]);
    assert_eq!(steps.returned_at_drop, [1, 0, 0, 1, 0, 1]);
}

/// A window takes the differences of two readings, the steps' words
/// wrapping as the profile's do.
#[test]
fn a_window_of_the_steps_is_the_difference_of_two_readings() {
    let counter = ProfileCounter::new();
    let mut tally = ProfileTally::new();
    tally.began(&counter, 0);
    tally.home(0, 1);
    tally.ended(&counter, 10);
    let start = counter.read_steps();
    tally.began(&counter, 100);
    tally.home(0, 2);
    tally.ended(&counter, 110);
    let window = TurnSteps::window(&start, &counter.read_steps());
    assert_eq!(window.home, [2, 1, 0, 0]);
    assert_eq!(TurnSteps::from_words(&window.words()), window);
    assert_eq!(TurnSteps::window(&window, &window), TurnSteps::NONE);
}

/// The actor's intakes count by their MSDUs, zero ones included, and the
/// station's computed costs by kind.
#[test]
fn intakes_count_by_their_msdus_and_costs_by_kind() {
    let intakes = IntakeCounter::new();
    for msdus in [0, 1, 2, 4, 8, 16, 17, 255] {
        intakes.intake(msdus);
    }
    let read = intakes.read();
    assert_eq!((read.intakes, read.taking), (8, 7));
    assert_eq!(read.msdus, 1 + 2 + 4 + 8 + 16 + 17 + 255);
    assert_eq!(read.msdus_by_intake, [1, 1, 1, 1, 1, 1, 2]);
    assert_eq!(Intakes::window(&read, &read), Intakes::NONE);
    let costs = AirCostCounter::new();
    costs.least();
    costs.least();
    costs.ceiling();
    assert_eq!(
        costs.read(),
        AirCosts {
            least: 2,
            ceiling: 1
        }
    );
}
