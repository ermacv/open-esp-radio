//! Bounded router observations shared by the station acceptance gates.
use oer_hil_scenario::Result;
use oer_lab_router::{
    mac::Mac,
    router,
    ssh::{CommandObservation, Ssh, Stderr, read_stderr},
};

pub(crate) struct Observer {
    child: oer_os_process::owned::Child,
    reader: Option<std::thread::JoinHandle<Result<String>>>,
    diagnostics: Option<std::thread::JoinHandle<Stderr>>,
    command: Option<CommandObservation>,
}
impl Observer {
    pub(crate) fn start(ssh: &Ssh, script: &str) -> Result<Self> {
        use std::io::Read;
        let mut child = ssh.start(script)?;
        let mut stdout = child.take_stdout().ok_or("observer stdout missing")?;
        let stderr = child.take_stderr().ok_or("observer stderr missing")?;
        let diagnostics = std::thread::spawn(move || read_stderr(stderr, |_| {}));
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let read = stdout.by_ref().take(131073).read_to_end(&mut bytes);
            read.map_err(|_| "observer read failed")?;
            if bytes.len() > 131072 {
                return Err("observer exceeded bound".into());
            }
            String::from_utf8(bytes).map_err(|_| "invalid observer encoding".into())
        });
        Ok(Self {
            child,
            reader: Some(reader),
            diagnostics: Some(diagnostics),
            command: Some(ssh.observe(script)),
        })
    }
    pub(crate) fn finish(mut self) -> Result<String> {
        self.child.kill()?;
        let status = self.child.try_wait()?;
        let diagnostics = self
            .diagnostics
            .take()
            .ok_or("observer stderr reader missing")?
            .join()
            .map_err(|_| "observer stderr reader failed")?;
        let failed = diagnostics.error.is_some();
        if let Some(command) = self.command.take() {
            command.finish(status, &diagnostics.text, diagnostics.error.as_deref());
        }
        if failed {
            return Err("observer stderr failed; see router command evidence".into());
        }
        self.reader
            .take()
            .ok_or("observer reader missing")?
            .join()
            .map_err(|_| "observer reader failed")?
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        if let Some(command) = self.command.take() {
            let diagnostics = self
                .diagnostics
                .take()
                .and_then(|reader| reader.join().ok());
            let (stderr, error) = diagnostics.map_or_else(
                || {
                    (
                        String::new(),
                        Some("observer stderr reader unavailable".to_owned()),
                    )
                },
                |diagnostics| (diagnostics.text, diagnostics.error),
            );
            command.finish(
                self.child.try_wait().ok().flatten(),
                &stderr,
                error
                    .as_deref()
                    .or(Some("observer stopped before completion")),
            );
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
/// Project only known hostapd events. A fresh subscription excludes old runs.
pub(crate) fn hostapd(ssh: &Ssh, target: Mac) -> Result<Observer> {
    let mac = target.octets().map(|b| format!("{b:02x}")).join(":");
    let pipeline = format!(
        "logread -f -l 1 | awk -v target={} 'NR>1 && index(tolower($0),target) {{label=\"\"; if(/pairwise key handshake completed/)label=\"pairwise-completed\"; else if(/group key handshake completed/)label=\"group-completed\"; else if(/group key handshake failed/)label=\"group-failed\"; else if(/AP-STA-CONNECTED/)label=\"authorized\"; else if(/AP-STA-DISCONNECTED/)label=\"disconnected\"; if(label!=\"\"){{print substr($0,1,24),label;fflush()}}}}'",
        router::quote(&mac)?
    );
    Observer::start(
        ssh,
        &format!("exec timeout 240 sh -c {}", router::quote(&pipeline)?),
    )
}
#[derive(Debug, serde::Serialize)]
pub(crate) struct Dhcp {
    pub broadcast: bool,
    pub kind: &'static str,
}
pub(crate) fn dhcp(text: &str) -> Vec<Dhcp> {
    let mut broadcast = false;
    text.lines()
        .filter_map(|line| {
            if line.contains("ethertype IPv4") {
                broadcast = line.contains(" > ff:ff:ff:ff:ff:ff,");
            }
            let kind = if line.contains("DHCP-Message") && line.contains(": Offer") {
                "offer"
            } else if line.contains("DHCP-Message") && line.contains(": ACK") {
                "ack"
            } else {
                return None;
            };
            Some(Dhcp { broadcast, kind })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dhcp_records_preserve_broadcast_without_payload_or_identity() {
        let rows = dhcp(
            "1.000000 02:00:00:00:00:01 > ff:ff:ff:ff:ff:ff, ethertype IPv4\n DHCP-Message (53), length 1: Offer\n2.000000 02:00:00:00:00:01 > 02:00:00:00:00:02, ethertype IPv4\n DHCP-Message (53), length 1: ACK\nsecret-key",
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[0].broadcast);
        assert!(!rows[1].broadcast);
        assert!(
            !serde_json::to_string(&rows)
                .expect("evidence")
                .contains("02:00")
        );
    }
}
