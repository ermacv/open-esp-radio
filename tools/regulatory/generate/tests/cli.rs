//! What a refused command line exits with, through the package's own binary.

use std::process::{Command, Stdio};

fn regdb() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_regulatory-generate"));
    command.stdin(Stdio::null()).stdout(Stdio::null());
    command
}

#[test]
fn a_refused_command_line_exits_two_with_its_usage() {
    let output = regdb().stderr(Stdio::piped()).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let usage = String::from_utf8(output.stderr).unwrap();
    assert!(
        usage.contains("Usage: ") && usage.contains("<db.txt>"),
        "{usage}"
    );
}

#[test]
fn a_refusal_that_cannot_be_written_is_what_the_exit_code_says() {
    // A pipe nobody reads refuses the usage message.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = regdb().stderr(writer).status().unwrap();
    assert_eq!(status.code(), Some(1));
}
