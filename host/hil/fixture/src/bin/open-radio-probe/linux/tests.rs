use super::*;
use std::{os::unix::net::UnixStream, time::Duration};

#[test]
fn prewritten_commands_do_not_hide_bytes_from_poll() {
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let control = Control::new(reader).unwrap();
    writer.write_all(b"configuration\nstart\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    assert_eq!(control.line(deadline).unwrap(), "configuration");
    assert_eq!(control.line(deadline).unwrap(), "start");
    drop(writer);
    assert!(
        control
            .line(deadline)
            .unwrap_err()
            .to_string()
            .contains("disconnected")
    );
}

#[test]
fn oversized_command_fails_without_waiting_for_its_terminator() {
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let control = Control::new(reader).unwrap();
    writer.write_all(&[b'x'; 257]).unwrap();
    assert!(
        control
            .line(Instant::now() + Duration::from_secs(1))
            .unwrap_err()
            .to_string()
            .contains("256")
    );
}
