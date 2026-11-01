use super::*;
use crate::power::HubPort;

fn entry(chip: &str) -> oer_lab_model::Board {
    oer_lab_model::Stand::parse(&format!(
        "schema = 2\n[[board]]\nid = \"b\"\nusb-serial = \"02:00:00:00:00:01\"\n\
         chip = \"{chip}\"\nreset = [\"jtag\", \"power\"]\n"
    ))
    .unwrap()
    .boards
    .remove(0)
}

fn powered(chip: &str) -> Board {
    Board::with_power(
        &entry(chip),
        PathBuf::from("/dev/serial/by-id/none"),
        Some(HubPower::new(HubPort {
            location: "1-1".to_owned(),
            port: 2,
        })),
    )
    .unwrap()
}

#[test]
fn a_board_without_a_chip_profile_keeps_its_hub_power() {
    let board = powered("esp32c5");
    assert!(board.has_power());
    assert_eq!(board.chip(), "esp32c5");
    assert!(board.require_power().is_ok());
}

#[test]
fn only_chip_dependent_operations_need_the_profile() {
    let repository = tempfile::tempdir().unwrap();
    std::fs::create_dir(repository.path().join("chips")).unwrap();
    let refused = powered("esp32c5")
        .profiled(repository.path())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("board `b`"), "{refused}");
    assert!(refused.contains("unsupported chip `esp32c5`"), "{refused}");
    assert!(refused.contains("hub port's power"), "{refused}");
}

#[test]
fn a_chip_with_a_profile_yields_its_profiled_board() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let board = powered("esp32s31").profiled(&root).unwrap();
    assert!(!board.espflash_chip.is_empty());
    assert!(board.has_power());
    let steps = board
        .rungs()
        .iter()
        .map(|rung| rung.step())
        .collect::<Vec<_>>();
    assert_eq!(steps, [RecoveryStep::JtagReset, RecoveryStep::PowerCycle]);
}
