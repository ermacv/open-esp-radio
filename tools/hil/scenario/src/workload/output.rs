//! Machine-readable stdout ownership for the host CLI.

use std::{
    fs::File,
    io::Write as _,
    sync::{Mutex, OnceLock},
};

use serde::Serialize;

use crate::workload::Result;

static MACHINE_STDOUT: OnceLock<Mutex<Box<dyn std::io::Write + Send>>> = OnceLock::new();

pub fn reserve_machine_stdout() -> Result<()> {
    #[cfg(unix)]
    let output: Box<dyn std::io::Write + Send> = {
        use std::os::fd::AsFd as _;

        let file = File::from(std::io::stdout().as_fd().try_clone_to_owned()?);
        // Inherited child stdout now reaches the diagnostic stderr stream.
        rustix::stdio::dup2_stdout(std::io::stderr().as_fd())?;
        Box::new(file)
    };
    #[cfg(not(unix))]
    let output: Box<dyn std::io::Write + Send> = Box::new(std::io::stdout());

    MACHINE_STDOUT
        .set(Mutex::new(output))
        .map_err(|_| "machine stdout was initialized more than once".into())
}

pub fn emit_json(value: &impl Serialize, pretty: bool) -> Result<()> {
    let output = MACHINE_STDOUT
        .get()
        .ok_or("machine stdout is not initialized")?;
    let mut output = output
        .lock()
        .map_err(|_| "machine stdout lock is poisoned")?;
    if pretty {
        serde_json::to_writer_pretty(&mut **output, value)?;
    } else {
        serde_json::to_writer(&mut **output, value)?;
    }
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
