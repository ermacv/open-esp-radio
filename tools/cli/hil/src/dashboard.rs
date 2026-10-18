//! `cargo hil dashboard`: a live page of the stand on this host.
//!
//! A single-threaded HTTP server bound to the loopback interface serves one
//! page and `status.json`: the arbiter's holders, queue, boards and recent
//! leases, plus the newest runs of the shared run store, each with the owner
//! and leases of the leases that name it. The page polls the JSON, so it
//! reflects the stand without reloading. The server only reads the arbiter's
//! state and the run manifests.
//!
//! One dashboard serves the host. It names itself in `dashboard.json` of the
//! arbiter directory: a second start of the same build prints the running
//! one's address, and a different build stops it and takes over. A running
//! dashboard exits once another replaced it, or once the stand's state has a
//! schema newer than it reads, instead of serving errors.
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
};

use oer_hil_run_bundle::RunStore;
use serde_json::{Value, json};

use crate::Result;

/// The port the page is bookmarked at; `--port` overrides it.
const DEFAULT_PORT: u16 = 8765;
const RECENT_LEASES: usize = 30;
const RECENT_RUNS: usize = 25;
const PAGE: &str = include_str!("dashboard.html");
/// How often the host fixtures are probed again.
const FIXTURE_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// The host fixtures as last probed, refreshed by a background thread.
static FIXTURES: std::sync::Mutex<Vec<oer_stand_fixtures::Fixture>> =
    std::sync::Mutex::new(Vec::new());

/// Probe the host fixtures now and every [`FIXTURE_REFRESH`] after.
fn watch_fixtures() {
    let Ok(lab) = oer_hil_lab::config::LabConfig::default_path() else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            let fixtures = oer_stand_fixtures::probe(&lab);
            *FIXTURES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = fixtures;
            std::thread::sleep(FIXTURE_REFRESH);
        }
    });
}

pub fn serve(store: &RunStore, args: &[std::ffi::OsString]) -> Result<std::process::ExitCode> {
    let port = match args {
        [] => DEFAULT_PORT,
        [flag, port] if flag == "--port" => port
            .to_str()
            .and_then(|port| port.parse().ok())
            .ok_or("--port takes a port number")?,
        _ => return Err("usage: cargo hil dashboard [--port PORT]".into()),
    };
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    let record = arbiter.directory().join("dashboard.json");
    let me = Instance::current(port)?;
    if let Some(running) = Instance::read(&record).filter(Instance::alive) {
        if running.build == me.build {
            eprintln!(
                "hil dashboard: already served at http://127.0.0.1:{}",
                running.port
            );
            return Ok(std::process::ExitCode::SUCCESS);
        }
        eprintln!(
            "hil dashboard: replacing the dashboard of another build (pid {})",
            running.pid
        );
        running.stop();
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|error| format!("cannot listen on 127.0.0.1:{port}: {error}"))?;
    me.write(&record)?;
    watch_fixtures();
    eprintln!(
        "hil dashboard: http://{} (Ctrl+C stops it)",
        listener.local_addr()?
    );
    listener.set_nonblocking(true)?;
    let mut checked = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                // A client that disconnects mid-request only loses its response.
                let _ = handle(stream, store);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {}
        }
        if oer_process::cancellation_requested() {
            me.withdraw(&record);
            eprintln!("hil dashboard: stopped");
            return Ok(std::process::ExitCode::SUCCESS);
        }
        if checked.elapsed() >= std::time::Duration::from_secs(2) {
            checked = std::time::Instant::now();
            if Instance::read(&record).is_none_or(|current| current.pid != me.pid) {
                eprintln!("hil dashboard: another dashboard took over; exiting");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            if arbiter.outdated() {
                eprintln!(
                    "hil dashboard: the stand's state is newer than this build reads; \
                     exiting. Start the dashboard of the current build."
                );
                let _ = std::fs::remove_file(&record);
                return Ok(std::process::ExitCode::SUCCESS);
            }
        }
    }
}

/// A running dashboard, as `dashboard.json` names it.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
struct Instance {
    pid: u32,
    /// When the process started, telling it from a later process with a
    /// reused PID.
    started_unix_millis: u64,
    port: u16,
    /// The executable and its modification time: another build differs.
    build: String,
}

impl Instance {
    fn current(port: u16) -> Result<Self> {
        let exe = std::env::current_exe()?;
        let modified = std::fs::metadata(&exe)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        Ok(Self {
            pid: std::process::id(),
            started_unix_millis: oer_process::proc::started_unix_millis(std::process::id())
                .ok_or("cannot read this process's start time")?,
            port,
            build: format!("{}@{modified}", exe.display()),
        })
    }

    fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    fn write(&self, path: &Path) -> Result<()> {
        oer_durable::atomic_write(path, &serde_json::to_vec(self)?)
    }

    fn alive(&self) -> bool {
        oer_process::proc::started_unix_millis(self.pid) == Some(self.started_unix_millis)
    }

    /// Remove `record` when it still names this dashboard, so a stopped
    /// dashboard leaves no record behind while another's stays.
    fn withdraw(&self, record: &Path) {
        if Instance::read(record).is_some_and(|current| current == *self) {
            let _ = std::fs::remove_file(record);
        }
    }

    /// Stop this dashboard and wait up to five seconds for its port.
    fn stop(&self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        for _ in 0..50 {
            if !self.alive() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn handle(mut stream: TcpStream, store: &RunStore) -> Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // Headers are not needed; drain them so the client sees a clean close.
    let mut header = String::new();
    while reader.read_line(&mut header)? > 2 {
        header.clear();
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, content_type, body) = respond(path, store);
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    Ok(())
}

fn respond(path: &str, store: &RunStore) -> (&'static str, &'static str, Vec<u8>) {
    match path.split('?').next().unwrap_or(path) {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.into()),
        path if path.starts_with("/runs/") => match run_file(store, &path["/runs/".len()..]) {
            Some((content_type, bytes)) => ("200 OK", content_type, bytes),
            None => (
                "404 Not Found",
                "text/plain",
                b"no such file in a run of the store".to_vec(),
            ),
        },
        path if path.starts_with("/jobs/") => match job_log(&path["/jobs/".len()..]) {
            Some(log) => ("200 OK", "text/plain; charset=utf-8", log.into_bytes()),
            None => ("404 Not Found", "text/plain", b"no such job log".to_vec()),
        },
        "/status.json" => match snapshot(store) {
            Ok(value) => (
                "200 OK",
                "application/json",
                serde_json::to_vec(&value).unwrap_or_default(),
            ),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                json!({"error": error.to_string()}).to_string().into_bytes(),
            ),
        },
        _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
    }
}

/// Whether `id` can only name an entry of its directory: digits, letters
/// and dashes, never a path.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `/runs/<id>/<path>`: a file of a run of the store, such as its
/// `report.html` and the artifacts the report links relative to it, with
/// its content type. Every path component is a plain name, and the file,
/// with symbolic links resolved, lies inside the run's directory.
fn run_file(store: &RunStore, rest: &str) -> Option<(&'static str, Vec<u8>)> {
    let (id, path) = rest.split_once('/')?;
    let plain = |name: &str| {
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !plain_id(id) || !path.split('/').all(plain) {
        return None;
    }
    let directory = store.run(id).canonicalize().ok()?;
    let file = directory.join(path).canonicalize().ok()?;
    if !file.starts_with(&directory) || !file.is_file() {
        return None;
    }
    Some((content_type(&file), std::fs::read(file).ok()?))
}

/// The content type a browser shows a run's file by: text inline, anything
/// unknown as bytes to save.
fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("csv") => "text/csv; charset=utf-8",
        Some("jsonl" | "log" | "txt" | "toml" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Bytes of a job log the page shows: its end, where a failure is.
const JOB_LOG_TAIL: usize = 256 * 1024;

/// `/jobs/<id>.log`: the end of an enqueued job's log.
fn job_log(rest: &str) -> Option<String> {
    let id = rest.strip_suffix(".log")?;
    if !plain_id(id) {
        return None;
    }
    let log = oer_stand_arbiter::Arbiter::open()
        .ok()?
        .jobs()
        .read(id)
        .ok()?
        .log?;
    let bytes = std::fs::read(log).ok()?;
    let start = bytes.len().saturating_sub(JOB_LOG_TAIL);
    Some(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

/// The stand and its newest runs.
fn snapshot(store: &RunStore) -> Result<Value> {
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    let status = arbiter.status()?;
    let jobs = arbiter.jobs().unfinished();
    let mut history = arbiter.history()?;
    let mut runs = oer_hil_analysis::dashboard::newest_runs(store, RECENT_RUNS);
    link_runs(&mut runs, &status.holders, &history);
    history.reverse();
    history.truncate(RECENT_LEASES);
    Ok(json!({
        "generated_unix": oer_durable::unix_seconds(),
        "hard_limit_secs": oer_stand_arbiter::HARD_LIMIT.as_secs(),
        "holders": status.holders,
        "queue": status.queue,
        "balances": status.balances,
        "devices": status.devices,
        "fixtures": *FIXTURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        "maintenance": status.maintenance,
        "jobs": oer_stand_arbiter::jobs::views(&jobs, &status),
        "leases": history,
        "runs": runs,
    }))
}

/// Name each run's owner and stand leases from the leases that name the run:
/// `owner`, `leases` (every lease ID, oldest first) and `holding` (the lease
/// it holds now). A run no lease names keeps a null owner.
fn link_runs(
    runs: &mut [Value],
    holders: &[oer_stand_arbiter::HolderStatus],
    history: &[oer_stand_arbiter::LeaseRecord],
) {
    for run in runs {
        let Some(id) = run["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let held = holders
            .iter()
            .find(|holder| holder.run.as_deref() == Some(id.as_str()));
        let past = history
            .iter()
            .filter(|record| record.run.as_deref() == Some(id.as_str()))
            .collect::<Vec<_>>();
        let owner = held
            .map(|holder| holder.owner.as_str())
            .or_else(|| past.last().map(|record| record.owner.as_str()));
        let leases = past
            .iter()
            .map(|record| record.id)
            .chain(held.map(|holder| holder.id))
            .collect::<Vec<_>>();
        run["owner"] = json!(owner);
        run["leases"] = json!(leases);
        run["holding"] = json!(held.map(|holder| holder.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(store: &RunStore, id: &str) {
        oer_hil_run_bundle::run::test_support::write_run(
            &store.run(id),
            1,
            oer_hil_run_bundle_format::run::RunState::Completed,
            Vec::new(),
            |_| {},
        );
    }

    #[test]
    fn a_run_link_reaches_only_files_inside_its_run() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        let id = "1790000000000-1f2e";
        run(&store, id);
        std::fs::write(store.run(id).join("report.html"), "<p>report</p>").unwrap();
        let artifacts = store.run(id).join("scenarios/a/repetition-001");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("uart.log"), "boot").unwrap();
        std::fs::write(store.runs().join("report.html"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            store.runs().join("report.html"),
            store.run(id).join("escape.html"),
        )
        .unwrap();
        assert_eq!(
            run_file(&store, &format!("{id}/report.html")),
            Some(("text/html; charset=utf-8", b"<p>report</p>".to_vec()))
        );
        // The artifacts a report links relative to itself.
        assert_eq!(
            run_file(&store, &format!("{id}/scenarios/a/repetition-001/uart.log")),
            Some(("text/plain; charset=utf-8", b"boot".to_vec()))
        );
        assert_eq!(run_file(&store, "../report.html"), None);
        assert_eq!(run_file(&store, &format!("{id}/../report.html")), None);
        assert_eq!(run_file(&store, &format!("{id}/scenarios")), None);
        assert_eq!(run_file(&store, &format!("{id}/escape.html")), None);
        assert_eq!(run_file(&store, &format!("{id}/missing.json")), None);
        assert_eq!(job_log("../../etc/passwd.log"), None);
        let (status, content_type, _) = respond(
            &format!("/runs/{id}/scenarios/a/repetition-001/uart.log"),
            &store,
        );
        assert_eq!(
            (status, content_type),
            ("200 OK", "text/plain; charset=utf-8")
        );
    }

    #[test]
    fn a_dashboard_record_names_a_live_process_of_one_build() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("dashboard.json");
        let me = Instance::current(8765).unwrap();
        me.write(&record).unwrap();
        let read = Instance::read(&record).unwrap();
        assert_eq!(read, me);
        assert!(read.alive());
        // A reused PID started at another time is not the dashboard.
        let recycled = Instance {
            started_unix_millis: me.started_unix_millis + 1_000,
            ..me.clone()
        };
        assert!(!recycled.alive());
        assert_eq!(Instance::current(9000).unwrap().build, me.build);
    }

    #[test]
    fn a_stopped_dashboard_withdraws_only_its_own_record() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("dashboard.json");
        let me = Instance::current(8765).unwrap();
        let other = Instance {
            pid: me.pid + 1,
            ..me.clone()
        };
        other.write(&record).unwrap();
        me.withdraw(&record);
        assert_eq!(Instance::read(&record), Some(other));
        me.write(&record).unwrap();
        me.withdraw(&record);
        assert!(!record.exists());
    }

    #[test]
    fn a_run_is_linked_to_the_leases_that_name_it() {
        let record = |id: u64, owner: &str, run: Option<&str>| oer_stand_arbiter::LeaseRecord {
            id,
            owner: owner.to_owned(),
            work: String::from("run a"),
            granted_unix: 1,
            released_unix: 2,
            outcome: oer_stand_arbiter::LeaseOutcome::Released,
            charged_ms: 0,
            balance_after_ms: 0,
            reason: None,
            preempted: None,
            scenarios: Vec::new(),
            run: run.map(str::to_owned),
            unknown: Default::default(),
        };
        let history = [
            record(1, "ble", Some("2-yielded")),
            record(2, "wifi", None),
            record(3, "ble", Some("1-done")),
        ];
        let holders = [oer_stand_arbiter::HolderStatus {
            id: 4,
            owner: String::from("ble"),
            work: String::from("run a"),
            pid: 1,
            elapsed_secs: 0,
            estimate_secs: 0,
            balance_ms: 0,
            claims: String::new(),
            job: None,
            run: Some(String::from("2-yielded")),
        }];
        let mut runs = [
            json!({"id": "2-yielded"}),
            json!({"id": "1-done"}),
            json!({"id": "0-unnamed"}),
        ];
        link_runs(&mut runs, &holders, &history);
        assert_eq!(
            runs,
            [
                json!({"id": "2-yielded", "owner": "ble", "leases": [1, 4], "holding": 4}),
                json!({"id": "1-done", "owner": "ble", "leases": [3], "holding": null}),
                json!({"id": "0-unnamed", "owner": null, "leases": [], "holding": null}),
            ]
        );
    }

    #[test]
    fn the_page_and_its_newest_runs_are_served() {
        let directory = tempfile::tempdir().unwrap();
        let store = RunStore::at(directory.path());
        run(&store, "1000-a");
        run(&store, "2000-b");
        run(&store, "3000-c");
        std::fs::create_dir_all(store.runs().join("runs.before-shared-store")).unwrap();
        let newest = oer_hil_analysis::dashboard::newest_runs(&store, 2);
        assert_eq!(
            newest
                .iter()
                .map(|run| run["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["3000-c", "2000-b"]
        );
        let (status, content_type, body) = respond("/", &store);
        assert_eq!(status, "200 OK");
        assert!(content_type.starts_with("text/html"));
        assert!(String::from_utf8(body).unwrap().contains("status.json"));
        assert_eq!(respond("/missing", &store).0, "404 Not Found");
    }
}
