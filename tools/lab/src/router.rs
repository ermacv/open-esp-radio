//! The stand router (OpenWrt), brought to W1's profile and verified over
//! SSH before every `run w1`. The router is a lab router: the profile is
//! committed and its previous settings are not restored.
//!
//! In order, each one SSH command ([`plan`] lists them):
//!
//! 1. `uci changes wireless`: staged changes of someone else would be
//!    committed with the profile, so any refuses the run;
//! 2. `uci show wireless`: the radio is a 2.4 GHz `wifi-device`, enabled;
//!    the AP section is an AP `wifi-iface` of that radio, not hidden; no
//!    other enabled `wifi-iface` of the radio carries the stand's SSID;
//! 3. the profile: on the radio `channel`, `htmode` [`HTMODE`] and
//!    `beacon_int` [`BEACON_INTERVAL_TU`], on the AP section `ssid` and
//!    `disabled=0`; `uci commit wireless`; `wifi reload`;
//! 4. `uci show wireless` again: the committed profile, and exactly one
//!    enabled `wifi-iface` of the radio with the stand's SSID;
//! 5. `iw dev <interface> info`, up to [`SETTLE_ATTEMPTS`] times
//!    [`SETTLE_PAUSE`] apart: an AP on the profile's channel, 20 MHz wide,
//!    with the stand's SSID; its `addr` is the stand's BSSID.
//!
//! Encryption and key are never set or read: `uci show` output keeps only the
//! options named in [`KEPT_OPTIONS`], and an SSID only as whether it is the
//! stand's. The SSH target, the interface, every SSID and the full BSSID
//! appear in no message, error or `Debug`; a command's output and stderr are
//! never shown (ssh's own errors name the host). Values reach the remote
//! shell single-quoted.
//!
//! [`Remote`] runs one command; [`crate::ssh::Ssh`] is the real one.

use std::{fmt, time::Duration};

pub use crate::ssh::{Remote, Result};
use crate::{mac::Mac, ssh::token};

/// The radio's `htmode`: HT 20 MHz, which every 802.11n radio (mt76
/// included, HE capable or not) accepts.
pub const HTMODE: &str = "HT20";
/// The radio's `beacon_int`, in TU.
pub const BEACON_INTERVAL_TU: u16 = 100;
/// How many times `iw` is asked after the reload.
pub const SETTLE_ATTEMPTS: u32 = 16;
/// The pause between two of those: 15 pauses, 30 s in all.
pub const SETTLE_PAUSE: Duration = Duration::from_secs(2);
/// The `uci show` options read; every other one (`key`, `encryption`, …) is
/// dropped unread.
pub const KEPT_OPTIONS: [&str; 10] = [
    "device",
    "mode",
    "disabled",
    "hidden",
    "ssid",
    "band",
    "hwmode",
    "channel",
    "htmode",
    "beacon_int",
];

/// A UCI section name: letters, digits and `_`.
fn section_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `text` as one word of the remote shell.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The profile W1 runs against.
pub struct Profile {
    radio: String,
    ap_section: String,
    interface: String,
    channel: u8,
    ssid: String,
}

impl fmt::Debug for Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Profile")
            .field("radio", &self.radio)
            .field("ap_section", &self.ap_section)
            .field("channel", &self.channel)
            .finish_non_exhaustive()
    }
}

impl Profile {
    /// The profile of `radio` (the `wifi-device` section) and `ap_section`
    /// (the `wifi-iface`), whose network device is `interface`, on
    /// `channel` with `ssid`.
    pub fn new(
        radio: &str,
        ap_section: &str,
        interface: &str,
        channel: u8,
        ssid: &str,
    ) -> Result<Self> {
        for (name, value) in [
            ("[station_fixture].radio", radio),
            ("[station_fixture].ap_section", ap_section),
        ] {
            if !section_name(value) {
                return Err(format!(
                    "{name} is no named UCI section (letters, digits and `_`)"
                ));
            }
        }
        if !token(interface) {
            return Err(
                "[station_fixture].wireless_interface holds characters other than letters, \
                 digits, `.`, `_`, `-` and `@`"
                    .to_owned(),
            );
        }
        if !(1..=13).contains(&channel) {
            return Err(format!(
                "[station_fixture].channel {channel} is no 2.4 GHz channel 1 to 13"
            ));
        }
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(format!(
                "[station].ssid is {} bytes, not 1 to 32",
                ssid.len()
            ));
        }
        if ssid.chars().any(char::is_control) {
            return Err("[station].ssid holds a control character".to_owned());
        }
        Ok(Self {
            radio: radio.to_owned(),
            ap_section: ap_section.to_owned(),
            interface: interface.to_owned(),
            channel,
            ssid: ssid.to_owned(),
        })
    }

    /// The one line that may be shown of the profile.
    pub fn describe(&self) -> String {
        format!(
            "radio {} and AP section {}: channel {}, {HTMODE}, beacon interval \
             {BEACON_INTERVAL_TU} TU",
            self.radio, self.ap_section, self.channel
        )
    }

    /// Step 3: the profile, committed and reloaded.
    fn apply_command(&self) -> String {
        let set = |section: &str, option: &str, value: &str| {
            format!(
                "uci set {}",
                quote(&format!("wireless.{section}.{option}={value}"))
            )
        };
        [
            set(&self.radio, "channel", &self.channel.to_string()),
            set(&self.radio, "htmode", HTMODE),
            set(&self.radio, "beacon_int", &BEACON_INTERVAL_TU.to_string()),
            set(&self.ap_section, "ssid", &self.ssid),
            set(&self.ap_section, "disabled", "0"),
            "uci commit wireless".to_owned(),
            "wifi reload".to_owned(),
        ]
        .join(" && ")
    }

    fn iw_command(&self) -> String {
        format!("iw dev {} info", quote(&self.interface))
    }
}

/// The remote commands in order, values as `<placeholders>`: what
/// [`apply`] runs, as the README lists them.
pub fn plan() -> Vec<String> {
    let profile = Profile {
        radio: "<radio>".to_owned(),
        ap_section: "<ap_section>".to_owned(),
        interface: "<wireless_interface>".to_owned(),
        channel: 0,
        ssid: "<ssid>".to_owned(),
    };
    vec![
        CHANGES.to_owned(),
        SHOW.to_owned(),
        profile
            .apply_command()
            .replace("channel=0", "channel=<channel>"),
        SHOW.to_owned(),
        format!(
            "{} (up to {SETTLE_ATTEMPTS} times, {} s apart)",
            profile.iw_command(),
            SETTLE_PAUSE.as_secs()
        ),
    ]
}

const CHANGES: &str = "uci changes wireless";
const SHOW: &str = "uci show wireless";

/// The applied profile: the stand AP's BSSID and channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Applied {
    pub bssid: Mac,
    pub channel: u8,
}

impl Applied {
    /// The one line that may be shown of it.
    pub fn describe(&self) -> String {
        format!("the stand AP {} is on channel {}", self.bssid, self.channel)
    }
}

/// Bring the router to `profile` and verify it; the stand AP's BSSID and
/// channel.
pub fn apply(remote: &mut dyn Remote, profile: &Profile) -> Result<Applied> {
    let changes = remote.run(CHANGES).map_err(step(CHANGES))?;
    refuse_staged_changes(&changes)?;
    let before = Wireless::parse(&remote.run(SHOW).map_err(step(SHOW))?, &profile.ssid)?;
    before.check_before(profile)?;
    remote
        .run(&profile.apply_command())
        .map_err(step("applying the profile"))?;
    let after = Wireless::parse(&remote.run(SHOW).map_err(step(SHOW))?, &profile.ssid)?;
    after.check_after(profile)?;
    settle(remote, profile)
}

/// An error of the step `what`.
fn step(what: &str) -> impl FnOnce(String) -> String + '_ {
    move |error| format!("router, {what}: {error}")
}

/// Step 5: `iw` until the AP runs the profile.
fn settle(remote: &mut dyn Remote, profile: &Profile) -> Result<Applied> {
    let command = profile.iw_command();
    let mut last = String::new();
    for attempt in 0..SETTLE_ATTEMPTS {
        if attempt > 0 {
            remote.pause(SETTLE_PAUSE);
        }
        let info = match remote.run(&command) {
            Ok(text) => parse_iw_info(&text, &profile.ssid),
            Err(error) => Err(error),
        };
        match info {
            Ok(info) => match info.mismatch(profile.channel) {
                None => {
                    return Ok(Applied {
                        bssid: info.bssid,
                        channel: info.channel,
                    });
                }
                Some(reason) => last = reason,
            },
            Err(error) => last = error,
        }
    }
    Err(format!(
        "router, iw dev <wireless_interface> info: the AP did not come up with the profile \
         within {} s: {last}",
        u64::from(SETTLE_ATTEMPTS - 1) * SETTLE_PAUSE.as_secs()
    ))
}

/// `uci changes wireless` prints nothing when nothing is staged; otherwise
/// the staged options are named (without their values).
fn refuse_staged_changes(changes: &str) -> Result<()> {
    let staged = changes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let path = line.split('=').next().unwrap_or_default();
            if path.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'@' | b'[' | b']')
            }) {
                path.to_owned()
            } else {
                "<unnamed change>".to_owned()
            }
        })
        .collect::<Vec<_>>();
    if staged.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the router has staged, uncommitted wireless changes that the profile's commit would \
         take with it: {}; commit or revert them on the router (`uci revert wireless`)",
        staged.join(", ")
    ))
}

/// One `wireless` section of `uci show`, holding only [`KEPT_OPTIONS`], its
/// SSID only as whether it is the stand's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Section {
    pub name: String,
    pub kind: String,
    pub options: Vec<(String, String)>,
    pub stand_ssid: bool,
    /// The kept options read so far, `ssid` included.
    seen: Vec<String>,
}

impl Section {
    fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(option, _)| option == name)
            .map(|(_, value)| value.as_str())
    }

    /// UCI's `disabled` is `1` to disable; absent or `0` is enabled.
    fn enabled(&self) -> bool {
        self.option("disabled") != Some("1")
    }
}

/// The `wireless` configuration as `uci show wireless` prints it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Wireless {
    pub sections: Vec<Section>,
}

/// One `uci show` value: `'…'` with `'\''` for a quote, or a list of such
/// words; `None` when it is not quoted so.
fn uci_value(text: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '\'') => quoted = false,
            (true, c) => current.push(c),
            (false, '\'') => {
                quoted = true;
                in_word = true;
            }
            (false, '\\') if chars.peek() == Some(&'\'') && in_word => {
                chars.next();
                current.push('\'');
            }
            (false, ' ') if in_word => {
                items.push(std::mem::take(&mut current));
                in_word = false;
            }
            _ => return None,
        }
    }
    if quoted {
        return None;
    }
    if in_word {
        items.push(current);
    }
    (!items.is_empty()).then_some(items)
}

impl Wireless {
    /// `text`, which `uci show wireless` printed; `ssid` is the stand's.
    /// A malformed line is named by its number, never quoted.
    pub fn parse(text: &str, ssid: &str) -> Result<Self> {
        let mut wireless = Self::default();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let bad = || {
                format!(
                    "uci show wireless: line {} is not `wireless.<section>[.<option>]=<value>`",
                    number + 1
                )
            };
            let (path, value) = line.split_once('=').ok_or_else(bad)?;
            let path = path.strip_prefix("wireless.").ok_or_else(bad)?;
            match path.split_once('.') {
                None => {
                    if path.is_empty()
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    {
                        return Err(bad());
                    }
                    wireless.sections.push(Section {
                        name: path.to_owned(),
                        kind: value.to_owned(),
                        ..Section::default()
                    });
                }
                Some((name, option)) => {
                    let section = wireless
                        .sections
                        .iter_mut()
                        .rev()
                        .find(|section| section.name == name)
                        .ok_or_else(bad)?;
                    if !KEPT_OPTIONS.contains(&option) {
                        continue;
                    }
                    if section.seen.iter().any(|seen| seen == option) {
                        return Err(format!(
                            "uci show wireless: line {} repeats {name}.{option}",
                            number + 1
                        ));
                    }
                    section.seen.push(option.to_owned());
                    let mut items = uci_value(value).ok_or_else(bad)?;
                    if option == "ssid" {
                        section.stand_ssid = items.as_slice() == [ssid];
                        continue;
                    }
                    if items.len() != 1 {
                        return Err(format!(
                            "uci show wireless: {name}.{option} is a list, not one value"
                        ));
                    }
                    section.options.push((option.to_owned(), items.remove(0)));
                }
            }
        }
        Ok(wireless)
    }

    fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.name == name)
    }

    /// The radio and the AP section, as the profile needs them.
    fn sections(&self, profile: &Profile) -> Result<(&Section, &Section)> {
        let radio = self
            .section(&profile.radio)
            .filter(|section| section.kind == "wifi-device")
            .ok_or_else(|| format!("the router has no wifi-device section {}", profile.radio))?;
        let ap = self
            .section(&profile.ap_section)
            .filter(|section| section.kind == "wifi-iface")
            .ok_or_else(|| {
                format!(
                    "the router has no wifi-iface section {}",
                    profile.ap_section
                )
            })?;
        if ap.option("device") != Some(profile.radio.as_str()) {
            return Err(format!(
                "wifi-iface {} belongs to another radio than {}",
                profile.ap_section, profile.radio
            ));
        }
        if ap.option("mode") != Some("ap") {
            return Err(format!(
                "wifi-iface {} is not in mode ap",
                profile.ap_section
            ));
        }
        let band = matches!(
            (radio.option("band"), radio.option("hwmode")),
            (Some("2g"), _) | (None, Some("11b" | "11g"))
        );
        if !band {
            return Err(format!(
                "wifi-device {} is not a 2.4 GHz radio (band 2g)",
                profile.radio
            ));
        }
        if !radio.enabled() {
            return Err(format!("wifi-device {} is disabled", profile.radio));
        }
        if ap.option("hidden") == Some("1") {
            return Err(format!(
                "wifi-iface {} hides its SSID (hidden=1): W1 checks the SSID its beacons carry",
                profile.ap_section
            ));
        }
        Ok((radio, ap))
    }

    /// The other enabled `wifi-iface` sections of the radio with the
    /// stand's SSID, by name.
    fn others_with_ssid(&self, profile: &Profile) -> Vec<&str> {
        self.sections
            .iter()
            .filter(|section| {
                section.kind == "wifi-iface"
                    && section.name != profile.ap_section
                    && section.option("device") == Some(profile.radio.as_str())
                    && section.enabled()
                    && section.stand_ssid
            })
            .map(|section| section.name.as_str())
            .collect()
    }

    fn refuse_others(&self, profile: &Profile) -> Result<()> {
        let others = self.others_with_ssid(profile);
        if others.is_empty() {
            return Ok(());
        }
        Err(format!(
            "other enabled wifi-iface sections of {} carry the stand's SSID: {}; the stand \
             must be the only one (they are left as they are)",
            profile.radio,
            others.join(", ")
        ))
    }

    /// Step 2: the sections fit the profile.
    fn check_before(&self, profile: &Profile) -> Result<()> {
        self.sections(profile)?;
        self.refuse_others(profile)
    }

    /// Step 4: the committed profile.
    fn check_after(&self, profile: &Profile) -> Result<()> {
        let (radio, ap) = self.sections(profile)?;
        let channel = profile.channel.to_string();
        let beacon_int = BEACON_INTERVAL_TU.to_string();
        for (section, option, wanted) in [
            (radio, "channel", channel.as_str()),
            (radio, "htmode", HTMODE),
            (radio, "beacon_int", beacon_int.as_str()),
        ] {
            if section.option(option) != Some(wanted) {
                return Err(format!(
                    "the committed {}.{option} is {}, not the profile's {wanted}",
                    section.name,
                    section.option(option).unwrap_or("unset")
                ));
            }
        }
        if !ap.stand_ssid {
            return Err(format!(
                "the committed {}.ssid is not the stand's SSID",
                ap.name
            ));
        }
        if !ap.enabled() {
            return Err(format!("the committed {} is disabled", ap.name));
        }
        self.refuse_others(profile)
    }
}

/// What `iw dev <interface> info` says of the AP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IwInfo {
    pub bssid: Mac,
    pub channel: u8,
    pub width: Width,
    pub ap: bool,
    /// Whether its `ssid` line is the stand's SSID.
    pub stand_ssid: bool,
}

/// A channel's width as `iw` prints it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Width {
    /// `20 MHz`: HT20.
    Ht20,
    /// `20 MHz (no HT)`.
    NoHt,
    /// Any other width.
    Other,
}

impl IwInfo {
    /// Why it is not yet the profile on `channel`.
    fn mismatch(&self, channel: u8) -> Option<String> {
        if !self.ap {
            Some("the interface is not of type AP".to_owned())
        } else if self.channel != channel {
            Some(format!(
                "the AP is on channel {}, the profile's is {channel}",
                self.channel
            ))
        } else if self.width != Width::Ht20 {
            Some(format!("the AP's width is {:?}, not HT 20 MHz", self.width))
        } else if !self.stand_ssid {
            Some("the AP does not carry the stand's SSID".to_owned())
        } else {
            None
        }
    }
}

/// An SSID as `iw` prints it: `\xHH` for a byte it does not print.
fn iw_ssid(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut ssid = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'\\' {
            let hex = bytes.get(index + 1..index + 4)?;
            if hex.first() != Some(&b'x') {
                return None;
            }
            let digits = std::str::from_utf8(hex.get(1..)?).ok()?;
            ssid.push(u8::from_str_radix(digits, 16).ok()?);
            index += 4;
        } else {
            ssid.push(byte);
            index += 1;
        }
    }
    Some(ssid)
}

/// `channel <n> (<f> MHz), width: <w>, …`.
fn iw_channel(rest: &str) -> Option<(u8, Width)> {
    let (number, rest) = rest.split_once(' ')?;
    let channel = number.parse::<u8>().ok()?;
    let frequency = rest
        .strip_prefix('(')?
        .split_once(" MHz)")?
        .0
        .parse::<u32>()
        .ok()?;
    if !(1..=13).contains(&channel) || frequency != 2407 + 5 * u32::from(channel) {
        return None;
    }
    let width = rest.split_once("width: ")?.1;
    let width = width.split(',').next()?.trim();
    Some((
        channel,
        match width {
            "20 MHz" => Width::Ht20,
            "20 MHz (no HT)" => Width::NoHt,
            _ => Width::Other,
        },
    ))
}

/// What `iw dev <interface> info` printed; `ssid` is the stand's. Lines
/// other than `addr`, `type`, `ssid` and `channel` are ignored; nothing of
/// the text is quoted in an error.
pub fn parse_iw_info(text: &str, ssid: &str) -> Result<IwInfo> {
    let mut bssid = None;
    let mut channel = None;
    let mut ap = false;
    let mut stand_ssid = false;
    for line in text.lines() {
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "addr" => {
                let address =
                    Mac::parse(rest).ok_or("iw: the addr line holds no unicast MAC address")?;
                if bssid.replace(address).is_some() {
                    return Err("iw: two addr lines".to_owned());
                }
            }
            "channel" => {
                let parsed = iw_channel(rest)
                    .ok_or("iw: the channel line is not `channel <1..13> (<MHz> MHz), width: …`")?;
                if channel.replace(parsed).is_some() {
                    return Err("iw: two channel lines".to_owned());
                }
            }
            "type" => ap = rest == "AP",
            "ssid" => stand_ssid = iw_ssid(rest).is_some_and(|bytes| bytes == ssid.as_bytes()),
            _ => {}
        }
    }
    let bssid = bssid.ok_or("iw: no addr line")?;
    let (channel, width) = channel.ok_or("iw: no channel line (is the AP up?)")?;
    Ok(IwInfo {
        bssid,
        channel,
        width,
        ap,
        stand_ssid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSID: &str = "Made Up 'Lab' Net";
    const TARGET: &str = "made-up-router.lab";
    const INTERFACE: &str = "phy7-ap3";
    const BSSID: &str = "02:5e:c0:de:f6:d0";

    fn profile() -> Profile {
        Profile::new("radio9", "lab_ap", INTERFACE, 6, SSID).unwrap()
    }

    /// `uci show wireless` of a router with the stand's section and others.
    fn show(channel: &str, htmode: &str, ap_ssid: &str, extra: &str) -> String {
        format!(
            "wireless.radio9=wifi-device\n\
             wireless.radio9.type='mac80211'\n\
             wireless.radio9.path='platform/soc/18000000.wifi'\n\
             wireless.radio9.band='2g'\n\
             wireless.radio9.channel='{channel}'\n\
             wireless.radio9.htmode='{htmode}'\n\
             wireless.radio9.beacon_int='100'\n\
             wireless.radio9.cell_density='0'\n\
             wireless.radio8=wifi-device\n\
             wireless.radio8.band='5g'\n\
             wireless.lab_ap=wifi-iface\n\
             wireless.lab_ap.device='radio9'\n\
             wireless.lab_ap.network='lan'\n\
             wireless.lab_ap.mode='ap'\n\
             wireless.lab_ap.ssid='{ap_ssid}'\n\
             wireless.lab_ap.encryption='psk2'\n\
             wireless.lab_ap.key='made-up-passphrase'\n\
             wireless.five=wifi-iface\n\
             wireless.five.device='radio8'\n\
             wireless.five.mode='ap'\n\
             wireless.five.ssid='Made Up '\\''Lab'\\'' Net'\n\
             {extra}"
        )
    }

    fn iw(channel: u8, width: &str, ssid: &str) -> String {
        format!(
            "Interface {INTERFACE}\n\
             \tifindex 12\n\
             \twdev 0x2\n\
             \taddr {BSSID}\n\
             \tssid {ssid}\n\
             \ttype AP\n\
             \twiphy 0\n\
             \tchannel {channel} ({} MHz), width: {width}, center1: {} MHz\n\
             \ttxpower 20.00 dBm\n",
            2407 + 5 * u32::from(channel),
            2407 + 5 * u32::from(channel)
        )
    }

    /// iw escapes the quote-free SSID's inner spaces as they are.
    const IW_SSID: &str = "Made Up 'Lab' Net";

    /// Canned answers, matched by the command's first words; records every
    /// command.
    struct Canned {
        answers: Vec<(&'static str, Vec<Result<String>>)>,
        commands: Vec<String>,
        pauses: u32,
    }

    impl Canned {
        fn new(answers: Vec<(&'static str, Vec<Result<String>>)>) -> Self {
            Self {
                answers,
                commands: Vec::new(),
                pauses: 0,
            }
        }
    }

    impl Remote for Canned {
        fn run(&mut self, command: &str) -> Result<String> {
            self.commands.push(command.to_owned());
            let (_, answers) = self
                .answers
                .iter_mut()
                .find(|(prefix, _)| command.starts_with(prefix))
                .expect("a canned answer");
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            }
        }
        fn pause(&mut self, _: Duration) {
            self.pauses += 1;
        }
    }

    fn good() -> Canned {
        Canned::new(vec![
            ("uci changes", vec![Ok(String::new())]),
            (
                "uci show",
                vec![
                    Ok(show("1", "HE40", "Old Net", "")),
                    Ok(show("6", "HT20", "Made Up '\\''Lab'\\'' Net", "")),
                ],
            ),
            ("uci set", vec![Ok(String::new())]),
            (
                "iw dev",
                vec![
                    Err("the remote command exited with 237".to_owned()),
                    Ok(iw(1, "40 MHz", "Old\\x20Net")),
                    Ok(iw(6, "20 MHz", IW_SSID)),
                ],
            ),
        ])
    }

    #[test]
    fn the_profile_is_applied_verified_and_settles() {
        let mut remote = good();
        let applied = apply(&mut remote, &profile()).unwrap();
        assert_eq!(applied.bssid.octets(), [0x02, 0x5e, 0xc0, 0xde, 0xf6, 0xd0]);
        assert_eq!(applied.channel, 6);
        assert_eq!(remote.pauses, 2);
        assert_eq!(
            remote.commands,
            [
                "uci changes wireless".to_owned(),
                "uci show wireless".to_owned(),
                "uci set 'wireless.radio9.channel=6' && uci set 'wireless.radio9.htmode=HT20' \
                 && uci set 'wireless.radio9.beacon_int=100' \
                 && uci set 'wireless.lab_ap.ssid=Made Up '\\''Lab'\\'' Net' \
                 && uci set 'wireless.lab_ap.disabled=0' && uci commit wireless && wifi reload"
                    .to_owned(),
                "uci show wireless".to_owned(),
                "iw dev 'phy7-ap3' info".to_owned(),
                "iw dev 'phy7-ap3' info".to_owned(),
                "iw dev 'phy7-ap3' info".to_owned(),
            ]
        );
    }

    #[test]
    fn quoting_survives_the_remote_shell() {
        for text in [SSID, "a b", "it's", "'", "$(reboot)", "a\\b", "`x`;rm"] {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote(text)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), text);
        }
    }

    #[test]
    fn the_plan_lists_placeholders_only() {
        let plan = plan();
        assert_eq!(plan.len(), 5);
        assert_eq!(
            plan[2],
            "uci set 'wireless.<radio>.channel=<channel>' && uci set 'wireless.<radio>.htmode=HT20' \
             && uci set 'wireless.<radio>.beacon_int=100' && uci set 'wireless.<ap_section>.ssid=<ssid>' \
             && uci set 'wireless.<ap_section>.disabled=0' && uci commit wireless && wifi reload"
        );
        assert_eq!(
            plan[4],
            "iw dev '<wireless_interface>' info (up to 16 times, 2 s apart)"
        );
        // The README lists the very plan.
        let readme = include_str!("../../fw/README.md");
        for command in &plan {
            assert!(
                readme.contains(command.as_str()),
                "README lacks `{command}`"
            );
        }
    }

    #[test]
    fn staged_changes_refuse_before_anything_is_set() {
        let mut remote = good();
        remote.answers[0].1 = vec![Ok(
            "wireless.radio9.txpower='5'\nwireless.lab_ap.key='made-up-passphrase'\n".to_owned(),
        )];
        let error = apply(&mut remote, &profile()).unwrap_err();
        assert!(
            error.contains("wireless.radio9.txpower, wireless.lab_ap.key"),
            "{error}"
        );
        assert!(!error.contains("made-up-passphrase"), "{error}");
        assert_eq!(remote.commands.len(), 1);
    }

    #[test]
    fn uci_show_keeps_only_the_named_options() {
        let escaped = SSID.replace('\'', "'\\''");
        let wireless = Wireless::parse(&show("6", "HT20", &escaped, ""), SSID).unwrap();
        let debug = format!("{wireless:?}");
        for secret in ["made-up-passphrase", "psk2", "Made Up", "Lab", "lan"] {
            assert!(!debug.contains(secret), "{secret}: {debug}");
        }
        let ap = wireless.section("lab_ap").unwrap();
        assert!(ap.stand_ssid);
        assert_eq!(ap.option("mode"), Some("ap"));
        assert_eq!(ap.option("key"), None);
        // The quote-escaped SSID of the 5 GHz section is the stand's too.
        assert!(wireless.section("five").unwrap().stand_ssid);
        assert!(
            !Wireless::parse(&show("6", "HT20", "Other", ""), SSID)
                .unwrap()
                .section("lab_ap")
                .unwrap()
                .stand_ssid
        );
    }

    #[test]
    fn malformed_uci_output_is_named_by_line_only() {
        for (text, line) in [
            ("garbage made-up-passphrase\n", 1),
            (
                "wireless.radio9=wifi-device\nwireless.radio9.channel=6\n",
                2,
            ),
            (
                "wireless.radio9=wifi-device\nwireless.radio9.channel='6\n",
                2,
            ),
            ("wireless.x.ssid='orphan'\n", 1),
            ("network.lan=interface\n", 1),
        ] {
            let error = Wireless::parse(text, SSID).unwrap_err();
            assert!(error.contains(&format!("line {line} ")), "{error}");
            assert!(
                !error.contains("made-up") && !error.contains("orphan"),
                "{error}"
            );
        }
        let error = Wireless::parse(
            "wireless.radio9=wifi-device\nwireless.radio9.channel='1' '6'\n",
            SSID,
        )
        .unwrap_err();
        assert!(error.contains("is a list"), "{error}");
    }

    #[test]
    fn the_sections_must_fit_before_anything_is_set() {
        let profile = profile();
        let check = |text: String| Wireless::parse(&text, SSID).unwrap().check_before(&profile);
        check(show("1", "HE40", "Old", "")).unwrap();
        for (extra, reason) in [
            (
                "wireless.radio9.disabled='1'\n",
                "wifi-device radio9 is disabled",
            ),
            ("wireless.lab_ap.hidden='1'\n", "hides its SSID"),
            (
                "wireless.guest=wifi-iface\nwireless.guest.device='radio9'\nwireless.guest.mode='ap'\nwireless.guest.ssid='Made Up '\\''Lab'\\'' Net'\n",
                "carry the stand's SSID: guest;",
            ),
        ] {
            let error = check(show("1", "HE40", "Old", extra)).unwrap_err();
            assert!(error.contains(reason), "{reason}: {error}");
            assert!(!error.contains("Made Up"), "{error}");
        }
        let error = check(
            show("1", "HE40", "Old", "")
                .replace("lab_ap.device='radio9'", "lab_ap.device='radio8'"),
        )
        .unwrap_err();
        assert!(error.contains("belongs to another radio"), "{error}");
        let error = Wireless::parse(
            &show("1", "HE40", "Old", "wireless.lab_ap.mode='sta'\n"),
            SSID,
        )
        .unwrap_err();
        assert!(error.contains("repeats lab_ap.mode"), "{error}");
        // A disabled duplicate is no second stand.
        check(show(
            "1",
            "HE40",
            "Old",
            "wireless.guest=wifi-iface\nwireless.guest.device='radio9'\nwireless.guest.ssid='Made Up '\\''Lab'\\'' Net'\nwireless.guest.disabled='1'\n",
        ))
        .unwrap();
        let error = Wireless::parse(&show("1", "HE40", "Old", "").replace("'2g'", "'5g'"), SSID)
            .unwrap()
            .check_before(&profile)
            .unwrap_err();
        assert!(error.contains("not a 2.4 GHz radio"), "{error}");
        let error = Wireless::parse("wireless.radio9=wifi-device\n", SSID)
            .unwrap()
            .check_before(&profile)
            .unwrap_err();
        assert!(error.contains("no wifi-iface section lab_ap"), "{error}");
    }

    #[test]
    fn the_committed_profile_is_checked() {
        let profile = profile();
        let ssid = "Made Up '\\''Lab'\\'' Net";
        let check = |text: String| Wireless::parse(&text, SSID).unwrap().check_after(&profile);
        check(show("6", "HT20", ssid, "")).unwrap();
        for (text, reason) in [
            (
                show("11", "HT20", ssid, ""),
                "radio9.channel is 11, not the profile's 6",
            ),
            (show("6", "HE20", ssid, ""), "radio9.htmode is HE20"),
            (
                show("6", "HT20", "Other", ""),
                "lab_ap.ssid is not the stand's SSID",
            ),
            (
                show("6", "HT20", ssid, "wireless.lab_ap.disabled='1'\n"),
                "lab_ap is disabled",
            ),
        ] {
            let error = check(text).unwrap_err();
            assert!(error.contains(reason), "{reason}: {error}");
        }
    }

    #[test]
    fn iw_info_parses_and_refuses_garbage() {
        let info = parse_iw_info(&iw(6, "20 MHz", IW_SSID), SSID).unwrap();
        assert_eq!(info.bssid.octets(), [0x02, 0x5e, 0xc0, 0xde, 0xf6, 0xd0]);
        assert_eq!((info.channel, info.width, info.ap), (6, Width::Ht20, true));
        assert!(info.stand_ssid);
        assert_eq!(info.mismatch(6), None);
        assert_eq!(
            parse_iw_info(&iw(6, "20 MHz (no HT)", IW_SSID), SSID)
                .unwrap()
                .width,
            Width::NoHt
        );
        // iw escapes what it does not print.
        let escaped = parse_iw_info(&iw(6, "20 MHz", "\\x20lead"), " lead").unwrap();
        assert!(escaped.stand_ssid);
        let other = parse_iw_info(&iw(11, "20 MHz", "Other"), SSID).unwrap();
        assert!(!other.stand_ssid);
        assert!(other.mismatch(6).unwrap().contains("channel 11"));
        let good = iw(6, "20 MHz", IW_SSID);
        for (text, reason) in [
            (
                good.replace("\taddr 02:5e:c0:de:f6:d0\n", ""),
                "no addr line",
            ),
            (
                good.replace("02:5e:c0:de:f6:d0", "03:5e:c0:de:f6:d0"),
                "no unicast MAC",
            ),
            (
                good.replace("02:5e:c0:de:f6:d0", "02:5e:c0:de:f6"),
                "no unicast MAC",
            ),
            (
                good.lines()
                    .filter(|line| !line.contains("channel"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                "no channel line",
            ),
            (
                good.replace("(2437 MHz)", "(2412 MHz)"),
                "the channel line is not",
            ),
            (
                good.replace("channel 6 ", "channel 36 "),
                "the channel line is not",
            ),
            (format!("{good}\taddr {BSSID}\n"), "two addr lines"),
            (
                "command failed: No such device (-19)\n".to_owned(),
                "no addr line",
            ),
            ("\u{0}\u{1}garbage\n".to_owned(), "no addr line"),
        ] {
            let error = parse_iw_info(&text, SSID).unwrap_err();
            assert!(error.contains(reason), "{reason}: {error}");
            assert!(
                !error.contains("02:5e") && !error.contains(INTERFACE),
                "{error}"
            );
        }
    }

    #[test]
    fn an_ap_that_never_settles_refuses_after_the_bounded_wait() {
        let mut remote = good();
        remote.answers[3].1 = vec![Ok(iw(1, "20 MHz", IW_SSID))];
        let error = apply(&mut remote, &profile()).unwrap_err();
        assert!(error.contains("within 30 s"), "{error}");
        assert!(error.contains("channel 1, the profile's is 6"), "{error}");
        assert_eq!(remote.pauses, SETTLE_ATTEMPTS - 1);
    }

    #[test]
    fn a_failing_ssh_is_an_error_without_the_target() {
        let mut remote = Canned::new(vec![(
            "uci",
            vec![Err(
                "ssh could not reach the router or was refused (exit 255; BatchMode needs a loaded key)"
                    .to_owned(),
            )],
        )]);
        let error = apply(&mut remote, &profile()).unwrap_err();
        assert!(
            error.starts_with("router, uci changes wireless: ssh could not"),
            "{error}"
        );
        assert_eq!(remote.commands.len(), 1);
    }

    #[test]
    fn values_are_validated_without_being_repeated() {
        for (radio, ap, interface, channel, ssid) in [
            ("radio 9", "lab_ap", INTERFACE, 6, SSID),
            ("radio9", "@wifi-iface[0]", INTERFACE, 6, SSID),
            ("radio9", "lab_ap", "phy7 ap3", 6, SSID),
            ("radio9", "lab_ap", INTERFACE, 14, SSID),
            ("radio9", "lab_ap", INTERFACE, 6, ""),
            (
                "radio9",
                "lab_ap",
                INTERFACE,
                6,
                "Made Up Lab Net Made Up Lab Net Made Up",
            ),
            ("radio9", "lab_ap", INTERFACE, 6, "Made\nUp"),
        ] {
            let error = Profile::new(radio, ap, interface, channel, ssid).unwrap_err();
            assert!(
                !error.contains("Made") && !error.contains("phy7"),
                "{error}"
            );
        }
    }

    #[test]
    fn nothing_secret_is_shown() {
        let profile = profile();
        let applied = apply(&mut good(), &profile).unwrap();
        let ssh = crate::ssh::Ssh::new(TARGET).unwrap();
        let info = parse_iw_info(&iw(6, "20 MHz", IW_SSID), SSID).unwrap();
        let shown = [
            format!("{profile:?}"),
            profile.describe(),
            format!("{applied:?}"),
            applied.describe(),
            format!("{ssh:?}"),
            format!("{info:?}"),
            plan().join("\n"),
        ]
        .join("\n");
        for secret in [
            SSID,
            "Made Up",
            TARGET,
            INTERFACE,
            BSSID,
            "02:5E:C0:DE:F6:D0",
            "5e:c0",
        ] {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        assert!(shown.contains("xx:xx:xx:xx:F6:D0"), "{shown}");
    }
}
