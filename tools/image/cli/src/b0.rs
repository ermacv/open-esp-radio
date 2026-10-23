//! Physical B0 procedure over the product's H4 boundary and pinned DTM peer.
use crate::{command, image};
use oer_hil_link::peer::{Expected, PeerConsole, SerialLink};
use oer_image_build::{Check, Enforced, Result, esp_idf::catalog};
use oer_lab_arbiter::{Arbiter, Request, claims::Claim};
use oer_lab_board::stand::{Board, Via};
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RESET: u16 = 0x0c03;
const RAND: u16 = 0x2018;
const RX: u16 = 0x201d;
const TX: u16 = 0x201e;
const END: u16 = 0x201f;
const RX2: u16 = 0x2033;
const STOP: u16 = 0xfc00;
const CHANNEL: u8 = 19;
const TIMEOUT: Duration = Duration::from_secs(10);
const AIR_INTERVAL: Duration = Duration::from_millis(500);

struct H4<S> {
    serial: S,
    log: fs::File,
}
impl<S: Read + Write> H4<S> {
    fn command(&mut self, opcode: u16, parameters: &[u8]) -> Result<Vec<u8>> {
        let [low, high] = opcode.to_le_bytes();
        let size = u8::try_from(parameters.len())?;
        let mut command = vec![1, low, high, size];
        command.extend_from_slice(parameters);
        writeln!(self.log, "tx {}", hex::encode(&command))?;
        self.serial.write_all(&command)?;
        self.serial.flush()?;
        let deadline = Instant::now() + TIMEOUT;
        let mut event = Vec::new();
        loop {
            oer_os_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err(format!("HCI {opcode:#06x} timed out").into());
            }
            let mut byte = [0];
            match self.serial.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            writeln!(self.log, "rx {:02x}", byte[0])?;
            if event.is_empty() && byte[0] != 4 {
                continue;
            } // ROM text before H4.
            event.push(byte[0]);
            if event.len() >= 3 && event.len() == usize::from(event[2]) + 3 {
                return completion(&event, opcode);
            }
        }
    }
    fn end(&mut self) -> Result<u16> {
        let returned = self.command(END, &[])?;
        let bytes: [u8; 2] = returned
            .try_into()
            .map_err(|_| "LE Test End did not return a packet count")?;
        Ok(u16::from_le_bytes(bytes))
    }
}
fn completion(event: &[u8], opcode: u16) -> Result<Vec<u8>> {
    let [low, high] = opcode.to_le_bytes();
    if event.len() < 7
        || event.get(..2) != Some(&[4, 0x0e])
        || usize::from(event[2]) + 3 != event.len()
        || event[3..6] != [1, low, high]
    {
        return Err("HCI completion has a wrong kind, extent, credit or opcode".into());
    }
    if event[6] != 0 {
        return Err(format!("HCI {opcode:#06x} status={:#04x}", event[6]).into());
    }
    Ok(event[7..].to_vec())
}
fn peer_end<L: oer_hil_link::peer::PeerLink>(peer: &mut PeerConsole<L>) -> Result<u64> {
    let reports = peer.command("END", "END", TIMEOUT, &["END"])?;
    let [report] = reports.as_slice() else {
        return Err("peer did not return one END report".into());
    };
    Ok(report
        .field("packets")
        .ok_or("peer END has no packet count")?
        .parse()?)
}

pub fn run(root: &Path, selector: &str, peer_selector: &str) -> Result<ExitCode> {
    let _signals = oer_os_process::install_signal_handlers()?;
    let stand = oer_lab_model::Stand::load(&oer_lab_model::path()?)?;
    let selected = stand.resolve(selector)?;
    let peer_selected = stand.resolve(peer_selector)?;
    if selected.chip != "esp32s31" || peer_selected.chip != "esp32c5" {
        return Err("B0 requires an S31 DUT and the pinned C5 DTM peer".into());
    }
    // This stand procedure uses an explicit 0 dBm cap and the actual lab
    // router's regulatory country, just as the Wi-Fi image procedures do.
    let (_, country) = command::w0_country(&stand)?;
    let bundle = image::build(
        root,
        "b0",
        Some(&image::Stand::B0 {
            country,
            cap_dbm: 0,
        }),
        false,
        Enforced::Only([Check::Frames, Check::Interrupts].into()),
    )?;
    let peer_build = catalog::build(root, "ble-dtm-peer")?;
    let entries = catalog::entries(root)?;
    let entry = catalog::entry(&entries, "ble-dtm-peer")?;
    let peer_bundle = catalog::bundle(root, entry, &peer_build, &root.join("target/fw/b0-peer"))?;
    let id = format!(
        "b0-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    );
    let output =
        oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Data, "evidence/371/b0-runs")?
            .join(&id);
    fs::create_dir_all(&output)?;
    let mut report = json!({"schema":1,"scope":"B0 physical H4/DTM","run_id":id,"board":selected.id,
        "peer":peer_selected.id,"country":String::from_utf8(country.to_vec())?,"cap_dbm":0,"channel":CHANNEL,
        "phy":"1M","interval_ms":AIR_INTERVAL.as_millis(),"minimum_packets":10,"boots":[],"passed":false,
        "source_commit":oer_os_process::git::text(root,["rev-parse","HEAD"])?,
        "source_dirty":!oer_os_process::git::text(root,["status","--porcelain"])?.is_empty(),
        "firmware":serde_json::from_slice::<serde_json::Value>(&fs::read(bundle.directory.join("bundle.json"))?)?,
        "peer_build":peer_build});
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut request = Request::from_environment("cargo fw b0: two-boot 1M DTM")?;
    request.run = Some(id.clone());
    request.claims = vec![
        Claim::board(&selected.mac),
        Claim::board(&peer_selected.mac),
        Claim::exclusive("air"),
    ];
    eprintln!("fw: {id}; waiting for both boards and exclusive air");
    let mut grant = Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(false);
    let target = Board::attached(root, &stand, selector)?
        .lease(grant.device(&selected.device_id()?).ok_or("no DUT lease")?)?;
    let peer_board = Board::attached(root, &stand, peer_selector)?.lease(
        grant
            .device(&peer_selected.device_id()?)
            .ok_or("no peer lease")?,
    )?;
    let store = oer_lab_board::image::Store::open()?;
    target.flash(&store, &bundle, "b0", Via::Usb, &id)?;
    peer_board.flash(&store, &peer_bundle, "ble-dtm-peer", Via::Usb, &id)?;
    let transcript = oer_hil_link::peer::PeerTranscript::default();
    let link = SerialLink::new(peer_board.port(), Box::new(peer_board.open_console()?));
    let mut peer = PeerConsole::open_recorded(
        link,
        &transcript,
        Expected {
            peer: "C5 DTM",
            protocol: 1,
            stack: None,
            restore: "cargo hil firmware flash ble-dtm-peer --board c5-a",
        },
        TIMEOUT,
    )?;
    let result = (|| -> Result<()> {
        for boot in 0..2 {
            let serial = target.reset_into_console()?;
            let mut hci = H4 {
                serial,
                log: fs::File::create(output.join(format!("boot-{boot}-h4.log")))?,
            };
            std::thread::sleep(Duration::from_millis(250));
            hci.command(RESET, &[])?;
            let random = hci.command(RAND, &[])?;
            if random.len() != 8 {
                return Err("LE Rand returned a wrong extent".into());
            }
            peer.command("RX", &format!("RX 1 {CHANNEL} 1M"), TIMEOUT, &[])?;
            std::thread::sleep(AIR_INTERVAL);
            let peer_idle = peer_end(&mut peer)?;
            hci.command(RX, &[CHANNEL])?;
            std::thread::sleep(AIR_INTERVAL);
            let dut_idle = hci.end()?;
            peer.command("RX", &format!("RX 1 {CHANNEL} 1M"), TIMEOUT, &[])?;
            hci.command(TX, &[CHANNEL, 37, 0])?;
            std::thread::sleep(AIR_INTERVAL);
            hci.end()?;
            let to_peer = peer_end(&mut peer)?;
            hci.command(RX2, &[CHANNEL, 1, 0])?;
            peer.command("TX", &format!("TX 2 {CHANNEL} 37 0 1M"), TIMEOUT, &[])?;
            std::thread::sleep(AIR_INTERVAL);
            let from_peer = hci.end()?;
            peer_end(&mut peer)?;
            hci.command(RESET, &[])?;
            hci.command(RAND, &[])?;
            hci.command(STOP, &[])?;
            report["boots"].as_array_mut().ok_or("boot report array")?.push(json!({"boot":boot,
                "peer_idle":peer_idle,"dut_idle":dut_idle,"to_peer":to_peer,"from_peer":from_peer,"ordinary_stop":true}));
            fs::write(
                output.join("manifest.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            if peer_idle != 0 || dut_idle != 0 || to_peer < 10 || from_peer < 10 {
                return Err("B0 quiet/count gate failed".into());
            }
        }
        Ok(())
    })();
    let cleanup = peer.command("RESET", "RESET", TIMEOUT, &[]);
    transcript.save(&output.join("peer.log"))?;
    if result.is_err() {
        let _ = target.reset_into_console();
    }
    report["error"] = result.as_ref().err().map(|e| e.to_string()).into();
    report["cleanup_error"] = cleanup.as_ref().err().map(|e| e.to_string()).into();
    report["passed"] = (result.is_ok() && cleanup.is_ok()).into();
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!("fw: B0 evidence {}", output.display());
    result?;
    cleanup?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completions_require_the_original_opcode_single_credit_and_exact_extent() {
        let good = [4, 0x0e, 4, 1, 3, 0x0c, 0];
        assert!(completion(&good, RESET).is_ok());
        for (index, value) in [(0, 1), (1, 0x0f), (2, 3), (3, 2), (4, 4), (5, 0x20), (6, 1)] {
            let mut bad = good;
            bad[index] = value;
            assert!(completion(&bad, RESET).is_err());
        }
        assert!(completion(&good[..6], RESET).is_err());
    }
}
