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
//! The harness knows no scenario, service or stored result. A case talks
//! to the device through [`Cx`] and decides its own verdict; the harness
//! prints each case's outcome as it is known and the device's diagnostic
//! lines, finishes the device under the lease, and exits with
//! the scenario's verdict: 0 when every case passed, 1 when one failed, 2
//! when one could not finish (the stand, the link, an unconfirmed request
//! or a cancellation) and none failed, or when the finish was not clean.
#![forbid(unsafe_code)]

use std::{process::ExitCode, time::Duration};

use oer_hil_link::{Boot, CallError, Completeness, Cursor, Health, Link, Received};
use oer_hil_protocol::{Endpoint, Key, Message};

mod dut;
#[cfg(test)]
mod fakes;
mod finish;
mod keys;
mod run;
mod stand;

pub use finish::Cleanup;
pub use oer_lab_client::protocol::RouterOperation;
pub use run::{Outcome, Verdict};

/// What a scenario needs from the stand and the device.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) image: &'static str,
    pub(crate) services: &'static [&'static [Key]],
    /// How its cases use the air: none (an image's radio works only on its
    /// command, so a scenario that never commands one leases the board
    /// alone), shared when they turn a radio on, exclusive when they
    /// measure it.
    pub(crate) air: Option<oer_lab_arbiter::claims::Mode>,
    /// Whether its cases use the lab router ([`Cx::router`]): its lease
    /// then holds the router and the air, shared.
    pub(crate) router: bool,
}

impl Scenario {
    /// A scenario named `name` whose device runs `image`
    /// (`oer_image_build::images`), which must serve every service of
    /// `services` (each its `Service::KEYS`).
    pub const fn new(
        name: &'static str,
        image: &'static str,
        services: &'static [&'static [Key]],
    ) -> Self {
        Self {
            name,
            image,
            services,
            air: None,
            router: false,
        }
    }

    /// This scenario, whose cases turn a radio on.
    pub const fn using_air(self) -> Self {
        Self {
            air: Some(oer_lab_arbiter::claims::Mode::Shared),
            ..self
        }
    }

    /// This scenario, whose cases measure the radio: no other radio work
    /// runs beside it.
    pub const fn using_air_exclusively(self) -> Self {
        Self {
            air: Some(oer_lab_arbiter::claims::Mode::Exclusive),
            ..self
        }
    }

    /// This scenario, whose cases use the lab router.
    pub const fn using_router(self) -> Self {
        Self {
            router: true,
            ..self
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }
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

/// A case's access to the device under test: its link, running boot and
/// resets.
pub struct Cx<'a> {
    dut: &'a mut (dyn dut::Device + 'a),
    boot: Boot,
    /// A request whose outcome is unknown: the device may still run it, so
    /// the boot takes no further request.
    unconfirmed: Option<u32>,
    /// What the case measured, in the order it measured it.
    measurements: Vec<(String, serde_json::Value)>,
}

impl<'a> Cx<'a> {
    fn new(dut: &'a mut (dyn dut::Device + 'a), boot: Boot) -> Self {
        Self {
            dut,
            boot,
            unconfirmed: None,
            measurements: Vec::new(),
        }
    }

    fn link(&self) -> Result<&Link, Failure> {
        self.dut
            .link()
            .ok_or_else(|| Failure::Incomplete("the device under test has no link".to_owned()))
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
        let Some(link) = self.dut.link() else {
            return Err(CallError::Link(oer_hil_link::LinkError::Transport(
                "the device under test has no link".to_owned(),
            )));
        };
        let result = link.call(&self.boot, request, timeout);
        if let Err(CallError::Unconfirmed { request_id }) = result {
            self.unconfirmed = Some(request_id);
        }
        result
    }

    /// [`Cx::request`] with its errors as the case's failure: a refusal, a
    /// payload of another type or another boot refute the case; a request
    /// that was not sent or got no response, a broken link or a
    /// cancellation leave it incomplete.
    pub fn call<E: Endpoint>(
        &mut self,
        request: E,
        timeout: Duration,
    ) -> Result<E::Response, Failure> {
        self.request(request, timeout).map_err(failure)
    }

    /// A cursor over the running boot's messages from now on: made before
    /// a request, it sees the request's events, also those that come
    /// before its response.
    pub fn events(&self) -> Result<Cursor, Failure> {
        Ok(self.link()?.cursor(&self.boot))
    }

    /// The first message `cursor` has not read that `matches` accepts,
    /// waiting up to `timeout`; `None` when none came. The cursor moves past
    /// the messages it refused; another cursor reads them on its own.
    pub fn take_message(
        &self,
        cursor: &mut Cursor,
        matches: impl Fn(&Received) -> bool,
        timeout: Duration,
    ) -> Result<Option<Received>, Failure> {
        self.link()?.take(cursor, matches, timeout).map_err(failure)
    }

    /// The first `M` `cursor` has not read, waiting up to `timeout`. None came:
    /// the case fails when the cursor's interval is complete and is
    /// incomplete when a message of it was lost, which may have been the
    /// `M`.
    pub fn event<M: Message>(&self, cursor: &mut Cursor, timeout: Duration) -> Result<M, Failure> {
        if let Some(message) = self.take_message(cursor, Received::is::<M>, timeout)? {
            return message
                .decode::<M>()
                .unwrap_or(Err(CallError::Payload { path: M::PATH }))
                .map_err(failure);
        }
        match self.completeness(cursor)? {
            Completeness::Complete => {
                Err(Failure::Fail(format!("no {} within {timeout:?}", M::PATH)))
            }
            Completeness::Lossy => Err(Failure::Incomplete(format!(
                "no {} within {timeout:?}, and a message of the interval was lost",
                M::PATH
            ))),
            Completeness::Ended { boot_id } => Err(Failure::Fail(format!(
                "the device rebooted into boot {boot_id:016x} before {}",
                M::PATH
            ))),
        }
    }

    /// Whether every message of `cursor`'s interval arrived and was kept.
    pub fn completeness(&self, cursor: &Cursor) -> Result<Completeness, Failure> {
        Ok(self.link()?.completeness(cursor))
    }

    /// What the link observed of the wire.
    pub fn health(&self) -> Result<Health, Failure> {
        Ok(self.link()?.health())
    }

    /// Record a measurement `name` of the case: printed at once, and kept in
    /// the run's results file (`target/hil/results/`) with the case's name.
    /// A measurement reports; it never decides the case.
    pub fn measure(&mut self, name: impl Into<String>, value: serde_json::Value) {
        let name = name.into();
        println!("  measure {name}: {value}");
        self.measurements.push((name, value));
    }

    /// Run `request` on the lab router under the run's lease (a scenario
    /// [`Scenario::using_router`]): its result as the operation returns it.
    /// A router that does not answer leaves the case incomplete.
    pub fn router(&self, request: RouterOperation) -> Result<serde_json::Value, Failure> {
        self.dut
            .router(request)
            .map_err(|error| Failure::Incomplete(format!("the lab router: {error}")))
    }

    /// Reset the device under test through its board and continue on the
    /// new boot of the same image; its earlier boot's requests end.
    pub fn reset(&mut self) -> Result<&Boot, Failure> {
        let boot = self.dut.reset().map_err(Failure::Incomplete)?;
        self.boot = boot;
        self.unconfirmed = None;
        Ok(&self.boot)
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
    let mut out = std::io::stdout();
    let mut measured = None;
    let (outcomes, cleanup) = match stand::Session::open(scenario, arguments.board.as_deref()) {
        Ok(mut session) => {
            let link = session.take_link();
            let (run, link) = {
                let board = session.board();
                let mut dut = dut::Dut::new(link, board, session.image_crc32());
                let run = run::cases(&mut dut, scenario, &selected, &mut out);
                (run, dut.into_link())
            };
            if !run.measurements.is_empty() {
                measured = Some((
                    session.root().to_owned(),
                    session.board_id().to_owned(),
                    run.measurements,
                ));
            }
            let cleanup = session.finish(link, run.ending);
            let cleanup = cleanup.released(session.release());
            (run.outcomes, Some(cleanup))
        }
        Err(error) => {
            let outcome = Outcome::stand(error.to_string());
            run::print(&outcome, &mut out);
            (vec![outcome], None)
        }
    };
    let verdict = run::summary(scenario, &outcomes, cleanup.as_ref(), &mut out);
    if let Some((root, board, measurements)) = measured {
        let mut results = run::results(scenario, &board, &outcomes, &measurements);
        if let Some(object) = results.as_object_mut() {
            object.insert(
                "cleanup".to_owned(),
                cleanup
                    .as_ref()
                    .and_then(Cleanup::problem)
                    .map_or(serde_json::Value::from("clean"), serde_json::Value::from),
            );
            object.insert("verdict".to_owned(), verdict_name(&verdict).into());
        }
        write_results(&root, scenario, &results);
    }
    ExitCode::from(verdict.exit_code())
}

fn verdict_name(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Fail(_) => "fail",
        Verdict::Incomplete(_) => "incomplete",
    }
}

/// Write `results` to `target/hil/results/<scenario>-<time>.json` of the
/// checkout `root` and print where; a file that cannot be written is
/// reported, never the run's verdict. The time is in nanoseconds: two runs
/// never share a file.
fn write_results(root: &std::path::Path, scenario: &Scenario, results: &serde_json::Value) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let directory = root.join("target/hil/results");
    let path = directory.join(format!("{}-{nanos}.json", scenario.name));
    let written = std::fs::create_dir_all(&directory).and_then(|()| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write as _;
                writeln!(file, "{results:#}")
            })
    });
    match written {
        Ok(()) => println!("results: {}", path.display()),
        Err(error) => eprintln!(
            "hil: the results file {} was not written: {error}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests;
