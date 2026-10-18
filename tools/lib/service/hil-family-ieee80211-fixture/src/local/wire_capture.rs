//! Host Ethernet boundary paired with independent radio evidence.
//! Includes ARP so socket acceptance cannot be mistaken for wire transmission.
use crate::Result;
use crate::capture_process;
use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Capture {
    child: capture_process::Capture,
    output: PathBuf,
}
impl Capture {
    pub fn start(
        interface: &str,
        target: Ipv4Addr,
        output: &Path,
        duration: Duration,
    ) -> Result<Self> {
        let path = output.join("host-wire.pcapng");
        let child =
            capture_process::dumpcap(interface, Some(&filter(target)), 128, &path, duration)?;
        Ok(Self {
            child,
            output: path,
        })
    }
    pub fn finish(self) -> Result<()> {
        let result = self.child.finish()?;
        let summary = String::from_utf8(result.stderr)?;
        fs::write(self.output.with_extension("log"), &summary)?;
        if !result.status.success() {
            return Err("host Ethernet capture failed".into());
        }
        let frames = crate::local::air_monitor::dumpcap_captured(&summary)?;
        let drops = crate::local::air_monitor::dumpcap_dropped(&summary)?;
        fs::write(
            self.output.with_extension("json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1, "captured_frames": frames, "kernel_dropped": drops,
                "boundary": "host packet socket; not proof of physical Ethernet delivery"
            }))?,
        )?;
        if frames == 0 || drops != 0 {
            return Err("host Ethernet capture is incomplete".into());
        }
        Ok(())
    }
}
fn filter(target: Ipv4Addr) -> String {
    format!("arp or (ip host {target} and udp)")
}
