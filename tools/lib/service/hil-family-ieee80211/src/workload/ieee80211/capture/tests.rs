use super::*;

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            duration: Duration::from_secs(31),
            ..capture_config()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            channel: Some(14),
            ..capture_config()
        }
        .validate()
        .is_err()
    );
}

fn capture_config() -> Config {
    Config {
        output: PathBuf::from("capture.pcapng"),
        timeout: Duration::from_secs(90),
        duration: Duration::from_secs(3),
        channel: None,
        snapshot_length: 256,
    }
}
