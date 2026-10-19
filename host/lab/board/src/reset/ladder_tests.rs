use super::*;
use std::cell::RefCell;

const FLASH_BOOT: &str = "rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)";
const DOWNLOAD: &str = "rst:0x1 (POWERON),boot:0x8 (DOWNLOAD(USB/UART0))";

/// A rung that answers with `banner`, or fails.
struct Fake {
    step: RecoveryStep,
    banner: std::result::Result<Option<&'static str>, &'static str>,
    tried: RefCell<usize>,
}

impl Fake {
    fn new(
        step: RecoveryStep,
        banner: std::result::Result<Option<&'static str>, &'static str>,
    ) -> Self {
        Self {
            step,
            banner,
            tried: RefCell::new(0),
        }
    }
}

impl Rung for Fake {
    fn step(&self) -> RecoveryStep {
        self.step
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        *self.tried.borrow_mut() += 1;
        self.banner
            .map(|banner| banner.map(str::to_owned))
            .map_err(Into::into)
    }
}

fn steps(ladder: &Ladder) -> Vec<RecoveryStep> {
    ladder.steps.iter().map(|step| step.step).collect()
}

#[test]
fn the_ladder_stops_at_the_first_rung_after_which_the_firmware_answers() {
    let rts = Fake::new(RecoveryStep::RtsReset, Ok(Some("garbled")));
    let jtag = Fake::new(RecoveryStep::JtagReset, Ok(None));
    let power = Fake::new(RecoveryStep::PowerCycle, Ok(None));
    let mut checks = 0;
    let ladder = climb(
        &[&rts, &jtag, &power],
        None,
        &|| FLASH_BOOT.into(),
        &mut || {
            checks += 1;
            checks == 2
        },
    );
    assert_eq!(
        steps(&ladder),
        [RecoveryStep::RtsReset, RecoveryStep::JtagReset]
    );
    assert_eq!(ladder.end, LadderEnd::Cleared);
    // A rung that read no console of its own reads it after the reset.
    assert_eq!(ladder.steps[1].outcome, Ok(Some(FLASH_BOOT.into())));
    assert_eq!(
        *power.tried.borrow(),
        0,
        "no rung after the one that cleared"
    );
}

#[test]
fn a_board_whose_firmware_never_answers_is_loadable_once_its_rom_answers() {
    let rts = Fake::new(RecoveryStep::RtsReset, Ok(Some("")));
    let jtag = Fake::new(
        RecoveryStep::JtagReset,
        Err("no OpenOCD was passed to the runner"),
    );
    let power = Fake::new(RecoveryStep::PowerCycle, Ok(None));
    let entry =
        || -> crate::Result<String> { Ok(format!("ESP-ROM\n{DOWNLOAD}\nwaiting for download")) };
    let ladder = climb(
        &[&rts, &jtag, &power],
        Some(&entry),
        &String::new,
        &mut || false,
    );
    assert_eq!(
        steps(&ladder),
        [
            RecoveryStep::RtsReset,
            RecoveryStep::JtagReset,
            RecoveryStep::PowerCycle,
            RecoveryStep::DownloadEntry
        ]
    );
    assert_eq!(
        ladder.steps[1].outcome,
        Err("no OpenOCD was passed to the runner".into())
    );
    assert_eq!(
        ladder.end,
        LadderEnd::Loadable {
            reset_line: DOWNLOAD.into()
        }
    );
}

#[test]
fn a_board_that_stays_silent_after_every_way_back_needs_a_person() {
    let rts = Fake::new(RecoveryStep::RtsReset, Ok(Some("")));
    let entry = || -> crate::Result<String> { Err("the board left USB but did not return".into()) };
    let ladder = climb(&[&rts], Some(&entry), &String::new, &mut || false);
    assert_eq!(ladder.end, LadderEnd::Silent);
    // Without a power rung there is no download entry either.
    let ladder = climb(&[&rts], None, &String::new, &mut || false);
    assert_eq!(steps(&ladder), [RecoveryStep::RtsReset]);
    assert_eq!(ladder.end, LadderEnd::Silent);
}

#[test]
fn a_rom_that_boots_from_flash_counts_as_loadable_too() {
    let rts = Fake::new(RecoveryStep::RtsReset, Ok(Some(FLASH_BOOT)));
    let ladder = climb(&[&rts], None, &String::new, &mut || false);
    assert_eq!(
        ladder.end,
        LadderEnd::Loadable {
            reset_line: FLASH_BOOT.into()
        }
    );
    assert!(
        crate::console::is_reset_line(DOWNLOAD) && !crate::console::is_reset_line("garbled output")
    );
}
