//! `oer-tool-lab-service --root REPOSITORY`: the LAB service of this host,
//! as its systemd user unit runs it (`cargo stand install`). REPOSITORY is
//! the clone it was built from: its chip profiles describe the boards.
#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let root = match arguments.as_slice() {
        [flag, root] if flag == "--root" => PathBuf::from(root),
        _ => {
            eprintln!("usage: oer-tool-lab-service --root REPOSITORY");
            return ExitCode::from(2);
        }
    };
    match oer_tool_lab_service::run::serve(root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lab-service: {error}");
            ExitCode::FAILURE
        }
    }
}
