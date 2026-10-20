//! `cargo hil`: the alias runs this binary with the command's arguments.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match oer_tool_hil_cli::command::run(&args) {
        Ok(status) => status,
        Err(error) => {
            if let Some(usage) = error.downcast_ref::<clap::Error>() {
                let _ = usage.print();
                return ExitCode::from(oer_tool_hil_cli::command::usage_status(usage, &args));
            }
            eprintln!("hil: {error}");
            ExitCode::from(failure_status(&*error))
        }
    }
}

fn failure_status(error: &(dyn std::error::Error + 'static)) -> u8 {
    if oer_os_process::is_cancelled(error) {
        130
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_cancelled_command_exits_130_and_any_other_failure_1() {
        assert_eq!(failure_status(&oer_os_process::Cancelled), 130);
        let failed: Box<dyn std::error::Error> = "no such run".into();
        assert_eq!(failure_status(&*failed), 1);
    }
}
