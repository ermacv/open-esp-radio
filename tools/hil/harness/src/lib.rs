//! The life cycle every HIL scenario program runs through.
//!
//! A scenario is an ordinary host program: it names the image its device
//! under test runs and the services that image must serve, and lists its
//! cases. [`main`] builds the image through the image pipeline before it
//! waits for the stand, leases the board, flashes the image unless the
//! board's receipt says it already runs it, opens the board's console with
//! a reset, checks that the running image is the built one (its Hello's
//! identity) and serves every required service,
//! runs the cases and releases the stand: the link first, then the board,
//! then the lease, also on an error or a cancellation.
//!
//! The harness knows no scenario, service or stored result. A case talks to
//! the device through [`Cx`] and decides its own verdict; the harness prints
//! each case's outcome and the device's diagnostic lines and exits with the
//! scenario's verdict: 0 when every case passed, 1 when one failed, 2 when
//! one could not finish (the stand, the link, an unconfirmed request or a
//! cancellation) and none failed.
#![forbid(unsafe_code)]

use std::{process::ExitCode, time::Duration};

use oer_hil_link::{Boot, CallError, Link};
use oer_hil_protocol::{Endpoint, Key};

mod keys;
mod run;
mod stand;

pub use run::{Outcome, Verdict};

/// What a scenario needs from the stand and the device.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub name: &'static str,
    /// The image the device under test runs (`oer_image_build::images`).
    pub image: &'static str,
    /// The keys of each service the image must serve: its `Service::KEYS`.
    pub services: &'static [&'static [Key]],
}

/// One case of a scenario: its name and what it runs.
#[derive(Clone, Copy)]
pub struct Case {
    pub name: &'static str,
    pub run: fn(&mut Cx<'_>) -> Result<(), Failure>,
}

/// Why a case did not pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    /// The device's behavior refutes the case.
    Fail(String),
    /// The case could not finish: its verdict is unknown.
    Incomplete(String),
}

/// `Ok` when `condition` holds, otherwise a failure naming `what`.
pub fn check(condition: bool, what: impl Into<String>) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure::Fail(what.into()))
    }
}

/// A case's access to the device under test: its link and running boot.
pub struct Cx<'a> {
    link: &'a Link,
    boot: Boot,
    /// A request whose outcome is unknown: the device may still run it, so
    /// the boot takes no further request.
    unconfirmed: Option<u32>,
}

impl<'a> Cx<'a> {
    fn new(link: &'a Link, boot: Boot) -> Self {
        Self {
            link,
            boot,
            unconfirmed: None,
        }
    }

    /// The running boot.
    pub fn boot(&self) -> &Boot {
        &self.boot
    }

    /// Send `request` and wait up to `timeout` for its response, as it came:
    /// for a case that checks a refusal itself.
    pub fn request<E: Endpoint>(
        &mut self,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, CallError> {
        if let Some(request_id) = self.unconfirmed {
            return Err(CallError::Unconfirmed { request_id });
        }
        let result = self.link.call(&self.boot, request, timeout);
        if let Err(CallError::Unconfirmed { request_id }) = result {
            self.unconfirmed = Some(request_id);
        }
        result
    }

    /// [`Cx::request`] with its errors as the case's failure: a refusal, a
    /// payload of another type or another boot refute the case; a request
    /// without a response, a broken link or a cancellation leave it
    /// incomplete.
    pub fn call<E: Endpoint>(
        &mut self,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, Failure> {
        self.request(request, timeout).map_err(failure)
    }
}

/// The failure a request's error makes of its case.
fn failure(error: CallError) -> Failure {
    match error {
        CallError::Rejected(_) | CallError::Payload { .. } | CallError::Rebooted { .. } => {
            Failure::Fail(error.to_string())
        }
        CallError::Unconfirmed { .. } | CallError::NotSent(_) | CallError::Link(_) => {
            Failure::Incomplete(error.to_string())
        }
    }
}

/// The command line every scenario program takes.
#[derive(clap::Parser)]
struct Arguments {
    /// The device under test, by its stand-file id or MAC; without it, the
    /// one board of the image's chip with the `dut` role.
    #[arg(long)]
    board: Option<String>,
    /// Run only this case. May be repeated.
    #[arg(long = "case")]
    cases: Vec<String>,
    /// Print the cases and exit.
    #[arg(long)]
    list: bool,
}

/// Run `scenario`'s `cases` on the stand and exit with its verdict.
pub fn main(scenario: &Scenario, cases: &[Case]) -> ExitCode {
    let arguments = <Arguments as clap::Parser>::parse();
    if arguments.list {
        for case in cases {
            println!("{}", case.name);
        }
        return ExitCode::SUCCESS;
    }
    let selected = match run::select(cases, &arguments.cases) {
        Ok(selected) => selected,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let _signals = match oer_os_process::install_signal_handlers() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("error: cannot install the signal handlers: {error}");
            return ExitCode::from(2);
        }
    };
    let outcomes = match stand::Session::open(scenario, arguments.board.as_deref()) {
        Ok(session) => run::cases(session.link(), scenario, session.image_crc32(), &selected),
        Err(error) => vec![Outcome::stand(error.to_string())],
    };
    let verdict = run::report(scenario, &outcomes, &mut std::io::stdout().lock());
    ExitCode::from(verdict.exit_code())
}

#[cfg(test)]
mod tests;
