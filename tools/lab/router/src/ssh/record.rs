//! Optional run evidence for SSH commands. Stdout is deliberately not logged:
//! configuration reads may contain unrelated credentials or identifiers.

use std::{
    io::{BufRead, BufReader, Read},
    process::ExitStatus,
    sync::Arc,
};
use zeroize::Zeroizing;

#[derive(Debug, serde::Serialize)]
pub struct CommandRecord {
    pub command: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr: String,
    pub error: Option<String>,
}

pub struct Stderr {
    pub text: String,
    pub error: Option<String>,
}

/// Retain observer diagnostics within a declared bound. Overflow/read errors
/// keep the captured prefix and explicitly invalidate the observation.
pub fn read_stderr(reader: impl Read, mut line: impl FnMut(&str)) -> Stderr {
    const LIMIT: usize = 128 * 1024;
    let mut reader = BufReader::new(reader.take(LIMIT as u64 + 1));
    let mut bytes = Vec::new();
    let mut error = None;
    loop {
        let start = bytes.len();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => break,
            Ok(_) => line(&String::from_utf8_lossy(&bytes[start..])),
            Err(_) => {
                error = Some("cannot read router stderr".to_owned());
                break;
            }
        }
    }
    if bytes.len() > LIMIT {
        error =
            Some("router stderr exceeded 131072-byte bound; captured prefix retained".to_owned());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(bytes) => {
            error = Some("router stderr is not UTF-8; replacement characters recorded".to_owned());
            String::from_utf8_lossy(bytes.as_bytes()).into_owned()
        }
    };
    Stderr { text, error }
}

/// One run's redaction and evidence sink, shared by its concurrent commands.
#[derive(Clone)]
pub struct CommandLog(Arc<Log>);
struct Log {
    secrets: Zeroizing<Vec<String>>,
    sink: Box<dyn Fn(CommandRecord) + Send + Sync>,
}
impl CommandLog {
    pub fn new(secrets: &[&str], sink: impl Fn(CommandRecord) + Send + Sync + 'static) -> Self {
        let mut forms = Vec::new();
        for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            forms.push((*secret).to_owned());
            if let Ok(quoted) = shlex::try_quote(secret) {
                forms.push(quoted.into_owned());
            }
        }
        // Replace longer quoted forms before their constituent raw values.
        forms.sort_by_key(|value| std::cmp::Reverse(value.len()));
        forms.dedup();
        Self(Arc::new(Log {
            secrets: Zeroizing::new(forms),
            sink: Box::new(sink),
        }))
    }
    fn redact(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in self.0.secrets.iter() {
            text = text.replace(secret, "<redacted>");
        }
        text
    }
}

/// Kept by a streaming observer until its owned process has terminated.
pub struct CommandObservation {
    log: Option<CommandLog>,
    script: Zeroizing<String>,
}
impl CommandObservation {
    pub(super) fn new(log: Option<CommandLog>, script: &str) -> Self {
        Self {
            log,
            script: Zeroizing::new(script.to_owned()),
        }
    }
    pub fn finish(self, status: Option<ExitStatus>, stderr: &str, error: Option<&str>) {
        let Some(log) = self.log else { return };
        #[cfg(unix)]
        let signal = status.and_then(|status| {
            use std::os::unix::process::ExitStatusExt;
            status.signal()
        });
        #[cfg(not(unix))]
        let signal = None;
        (log.0.sink)(CommandRecord {
            command: log.redact(&self.script),
            exit_code: status.and_then(|status| status.code()),
            signal,
            stderr: log.redact(stderr),
            error: error.map(|error| log.redact(error)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::process::ExitStatusExt, sync::Mutex};

    #[test]
    fn failures_keep_command_status_and_stderr_but_redact_all_secret_forms() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let output = records.clone();
        let secret = "generated ' secret";
        let log = CommandLog::new(&[secret, "oer-test-1234", ""], move |record| {
            output.lock().expect("records").push(record);
        });
        CommandObservation::new(
            Some(log.clone()),
            &format!(
                "set key={}\nset ssid=oer-test-1234",
                shlex::try_quote(secret).unwrap()
            ),
        )
        .finish(
            Some(ExitStatus::from_raw(2 << 8)),
            &format!("RTNETLINK: {secret}: No such file or directory\n"),
            None,
        );
        CommandObservation::new(Some(log), "ip -4 neigh show").finish(
            None,
            "",
            Some("SSH transport failed"),
        );
        let records = records.lock().expect("records");
        let text = serde_json::to_string(&*records).unwrap();
        assert!(!text.contains("generated"));
        assert!(!text.contains("oer-test-1234"));
        assert_eq!(records[0].exit_code, Some(2));
        assert_eq!(
            records[0].stderr,
            "RTNETLINK: <redacted>: No such file or directory\n"
        );
        assert_eq!(records[1].exit_code, None);
        assert_eq!(records[1].error.as_deref(), Some("SSH transport failed"));
    }

    #[test]
    fn streaming_diagnostics_retain_lines_and_make_overflow_explicit() {
        let text = b"listening on bridge\nRTNETLINK answers: No such file or directory\n";
        let mut lines = Vec::new();
        let stderr = read_stderr(text.as_slice(), |line| lines.push(line.to_owned()));
        assert_eq!(lines.len(), 2);
        assert_eq!(stderr.text.as_bytes(), text);
        assert!(stderr.error.is_none());
        let oversized = vec![b'x'; 131074];
        let stderr = read_stderr(oversized.as_slice(), |_| {});
        assert_eq!(stderr.text.len(), 131073);
        assert!(stderr.error.expect("overflow").contains("prefix retained"));
    }
}
