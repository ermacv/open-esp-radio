//! Passive smoke check. pcap is never written; decoded addresses stay private.
use oer_hil_lab::{
    air::AirCapture,
    config::{BoardChoice, LabConfig},
};
fn main() -> oer_hil_lab::Result<()> {
    let _signals = oer_os_process::install_signal_handlers()?;
    let lab = LabConfig::load(
        &LabConfig::default_path()?,
        "esp32s31",
        &BoardChoice {
            dut: Some("s31-a".to_owned()),
            ..Default::default()
        },
    )?;
    let channel: u8 = std::env::args()
        .nth(1)
        .ok_or("usage: air-capture CHANNEL")?
        .parse()?;
    let capture = AirCapture::start(&lab, channel)?;
    oer_os_process::sleep(std::time::Duration::from_secs(2))?;
    let frames = capture.finish()?;
    println!(
        "air: decoded {} management/ACK frames; no pcap file",
        frames.len()
    );
    Ok(())
}
