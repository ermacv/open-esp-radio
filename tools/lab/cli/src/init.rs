//! `cargo stand init [--force]`: write the user's stand file from the boards
//! attached to this host, as the LAB service detects them under stand
//! maintenance: one `[[board]]` per board with its chip and MAC, its radios
//! from the chip profile, no hubs. The file is private (mode 0600); an
//! existing one is kept unless `--force`.

use std::fmt::Write as _;
use std::path::Path;

use oer_chip_profile::Profile;

use oer_espressif_facts::{BluetoothMode, Start, WifiBand};
use oer_lab_model::Stand;

use crate::Result;

#[derive(clap::Parser)]
#[command(name = "cargo stand init", no_binary_name = true)]
pub(crate) struct InitCli {
    /// Replace an existing stand file.
    #[arg(long)]
    force: bool,
}

pub(crate) fn init(root: &Path, cli: InitCli) -> Result<std::process::ExitCode> {
    let path = oer_lab_model::paths::stand_file()?;
    if path.exists() && !cli.force {
        return Err(format!(
            "{} exists; `cargo stand init --force` replaces it",
            path.display()
        )
        .into());
    }
    let chips = Profile::all(root)?;
    // The LAB service asks each board's ROM, under stand maintenance: it
    // owns every board, also before the stand file names one.
    let owner = oer_lab_arbiter::owner::from_environment()?.to_string();
    let mut client = crate::service::client()?;
    client.acquire(
        oer_lab_client::protocol::LeaseRequest {
            owner: owner.clone(),
            work: String::from("stand init"),
            scenarios: Vec::new(),
            run: None,
            claims: vec![oer_lab_arbiter::claims::Claim::stand()],
            maintenance: Some(format!("stand init by {owner}")),
            divisible: false,
        },
        &mut |message| eprintln!("stand: {message}"),
    )?;
    let detected = crate::service::run(&mut client, oer_lab_client::protocol::Operation::Detect)?;
    let mut boards = Vec::new();
    for device in detected.as_array().ok_or("the service detected no list")? {
        let mac = device["mac"]
            .as_str()
            .ok_or("a detected board has no MAC")?;
        let chip = device["chip"]
            .as_str()
            .ok_or("a detected board has no chip")?;
        let profile = chips
            .iter()
            .find(|profile| profile.facts.id == chip)
            .ok_or_else(|| format!("board {mac} is a {chip}, which no chip profile names"))?;
        boards.push((mac.to_owned(), profile));
    }
    if boards.is_empty() {
        return Err("no board is attached; attach the stand's boards and run it again".into());
    }
    let text = stand_file(&host_name(), &boards);
    // What it writes is a stand file this build reads.
    let parsed = Stand::parse(&text)?;
    parsed.validate()?;
    parsed.validate_chips(&chips)?;
    write_private(&path, &text)?;
    // The service recovers the boards it reads to their end states.
    client.administer(oer_lab_client::protocol::Request::Reload)?;
    client.release()?;
    println!("wrote {} with {} board(s)", path.display(), boards.len());
    println!(
        "next: name each board as written on it, add the hubs and the fixture sections \
         (tools/lab/model/stand.example.toml), then `cargo stand discover` and `cargo stand doctor`"
    );
    Ok(std::process::ExitCode::SUCCESS)
}

/// The stand file of `boards` (MAC and chip profile) on the host `host`.
fn stand_file(host: &str, boards: &[(String, &Profile)]) -> String {
    let mut text = String::from(
        "# The HIL stand file `cargo stand init` wrote from the attached boards.\n\
         # Name each board as written on it; add the hubs, their ports and the\n\
         # fixture sections as tools/lab/model/stand.example.toml shows.\n\
         schema = 1\n\n[stand]\n",
    );
    let _ = writeln!(text, "id = {host:?}\nair = \"exclusive\"");
    let mut counts = std::collections::BTreeMap::<&str, u32>::new();
    for (mac, profile) in boards {
        let count = counts.entry(profile.facts.id.as_str()).or_default();
        *count += 1;
        let radios = radios(profile)
            .iter()
            .map(|radio| format!("{radio:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(
            text,
            "\n[[board]]\nid = \"{}-{}\"\nusb-serial = \"{mac}\"\nchip = \"{}\"\n\
             radios = [{radios}]\nroles = [\"dut\", \"peer\"]\nreset = [{}]\n",
            profile.facts.id,
            count,
            profile.facts.id,
            reset(profile)
                .iter()
                .map(|step| format!("{step:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    text
}

/// The radios a board of `profile`'s chip carries: all its chip's.
fn radios(profile: &Profile) -> Vec<&'static str> {
    let properties = &profile.facts.properties;
    let mut radios = Vec::new();
    for band in &properties.wifi_bands {
        radios.push(match band {
            WifiBand::Band2g4 => "wifi-2g4",
            WifiBand::Band5g => "wifi-5g",
        });
    }
    if properties.bluetooth.contains(&BluetoothMode::Le) {
        radios.push("ble");
    }
    if properties.ieee802154 {
        radios.push("ieee802154");
    }
    radios
}

/// The reset ladder of a board on no hub: an RTS reset through USB
/// Serial/JTAG where the chip's start policy allows one, then JTAG.
fn reset(profile: &Profile) -> Vec<&'static str> {
    match profile.facts.flash.as_ref().map(|flash| flash.start) {
        Some(Start::PowerOn) => vec!["jtag"],
        _ => vec!["usb-jtag-rts", "jtag"],
    }
}

/// This host's name, the stand's id.
fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "stand".to_owned())
}

/// Write `text` to `path`, readable by its owner only.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // A replaced file keeps the mode it had: hold it to 0600 too.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}
