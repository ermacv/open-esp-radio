//! The stand router (OpenWrt), brought to a HIL profile and verified over
//! SSH. The router is a lab router: the profile is committed and its
//! previous settings are not restored.
//!
//! Each step is one script on the router's `sh -s` ([`plan`] lists them,
//! the README explains them):
//!
//! 1. the staged wireless changes, as option paths only: any refuses the
//!    run, since the profile's commit would take them along;
//! 2. the wireless configuration, filtered on the router to the
//!    [`ALLOWED_OPTIONS`] of the `wifi-device` and `wifi-iface` sections:
//!    the radio is a 2.4 GHz `wifi-device`; the AP section an AP
//!    `wifi-iface` of it; no other enabled `wifi-iface` of the radio
//!    carries the stand's SSID;
//! 3. the radio's channels as its PHY lists them: the profile's channel is
//!    offered and neither disabled nor no-IR in the regulatory domain;
//! 4. the profile: on the radio `channel`, `htmode` [`HTMODE`],
//!    `beacon_int` [`BEACON_INTERVAL_TU`] and `disabled=0`, on the AP
//!    section `ssid`, `disabled=0` and `hidden=0`; `uci commit wireless`;
//!    `wifi reload`;
//! 5. the configuration again: the committed profile, and exactly one
//!    enabled `wifi-iface` of the radio with the stand's SSID;
//! 6. the AP interface, up to [`SETTLE_ATTEMPTS`] times [`SETTLE_PAUSE`]
//!    apart: an AP on the profile's channel, 20 MHz wide, with the stand's
//!    SSID; its `addr` is the stand's BSSID.
//!
//! The lower-MAC HIL profile leaves encryption unchanged. WPA2 station sets WPA2-PSK/CCMP without PMF and a short GTK rekey interval.
//! Keys are write-only: the generated passphrase reaches `uci batch` on
//! stdin, never an argument list, and secret buffers are zeroized. Every
//! read is filtered there by an allowlist, and the host refuses any line
//! outside it. An SSID is kept only as whether it is the stand's. The SSH
//! target, the interface, every SSID and the full BSSID appear in no
//! message, error or `Debug`. Optional run command evidence redacts secrets
//! and retains stderr; configuration stdout is never recorded.
//! Values reach the remote shell quoted by shlex, inside the script on stdin.

use std::{fmt, time::Duration};

use zeroize::Zeroizing;

pub use crate::ssh::{Remote, Result};
use crate::{mac::Mac, ssh::token};

/// The radio's `htmode`: HT 20 MHz, which every 802.11n radio (mt76
/// included, HE capable or not) accepts.
pub const HTMODE: &str = "HT20";
/// The radio's `beacon_int`, in TU.
pub const BEACON_INTERVAL_TU: u16 = 100;
/// How many times the AP interface is read after the reload.
pub const SETTLE_ATTEMPTS: u32 = 16;
/// The pause between two of those: 15 pauses, 30 s in all.
pub const SETTLE_PAUSE: Duration = Duration::from_secs(2);
/// The options of the wireless configuration that leave the router; every
/// other one (`key`, `sae_password`, `auth_secret`, `wpa_psk_file`, …) stays
/// there.
pub const ALLOWED_OPTIONS: [&str; 16] = [
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
    "country",
    "encryption",
    "ieee80211ax",
    "ieee80211r",
    "ieee80211w",
    "wpa_group_rekey",
];
/// The section types whose lines leave the router.
pub const ALLOWED_TYPES: [&str; 2] = ["wifi-device", "wifi-iface"];

/// A UCI section name: letters, digits and `_`.
fn section_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `text` as one word of the remote shell.
pub fn quote(text: &str) -> Result<String> {
    shlex::try_quote(text)
        .map(|word| word.into_owned())
        .map_err(|error| error.to_string())
}

/// Step 1: the staged changes, cut to their option paths on the router.
pub const CHANGES: &str =
    "changes=$(uci changes wireless) && printf '%s\\n' \"$changes\" | sed 's/=.*//'";

/// Step 2 and 5: the wireless configuration, filtered on the router to the
/// allowed section types and options.
pub fn show_script() -> String {
    format!(
        "config=$(uci show wireless) && printf '%s\\n' \"$config\" | grep -E \
         '^wireless\\.[^.=]+=({})$|^wireless\\.[^.=]+\\.({})='",
        ALLOWED_TYPES.join("|"),
        ALLOWED_OPTIONS.join("|")
    )
}

enum Security {
    Unchanged,
    Wpa2(Zeroizing<String>),
}

/// A run's wireless profile; WPA2 station uses the isolated station network.
pub struct Profile {
    radio: String,
    ap_section: String,
    interface: String,
    channel: u8,
    ssid: Zeroizing<String>,
    security: Security,
    group_rekey_seconds: u32,
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
            return Err(format!("the SSID is {} bytes, not 1 to 32", ssid.len()));
        }
        if ssid.chars().any(char::is_control) {
            return Err("the SSID holds a control character".to_owned());
        }
        Ok(Self {
            radio: radio.to_owned(),
            ap_section: ap_section.to_owned(),
            interface: interface.to_owned(),
            channel,
            ssid: Zeroizing::new(ssid.to_owned()),
            security: Security::Unchanged,
            group_rekey_seconds: crate::station_wpa2::GROUP_REKEY_SECONDS,
        })
    }

    /// WPA2 station's WPA2-Personal/CCMP HT20 profile, with PMF and HE disabled.
    /// Callers generate the SSID and passphrase with [`crate::credentials`]
    /// for each run. The passphrase is write-only and zeroized on drop.
    pub fn station_wpa2(
        radio: &str,
        ap_section: &str,
        interface: &str,
        channel: u8,
        ssid: &str,
        passphrase: &str,
    ) -> Result<Self> {
        if !(8..=63).contains(&passphrase.len())
            || !passphrase.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
        {
            return Err("the WPA2 passphrase must be 8 to 63 printable ASCII bytes".to_owned());
        }
        let mut profile = Self::new(radio, ap_section, interface, channel, ssid)?;
        profile.security = Security::Wpa2(Zeroizing::new(passphrase.to_owned()));
        Ok(profile)
    }

    fn station_network(&self) -> bool {
        !matches!(self.security, Security::Unchanged)
    }

    /// The one line that may be shown of the profile.
    pub fn describe(&self) -> String {
        format!(
            "radio {} and AP section {}: channel {}, {HTMODE}, beacon interval \
             {BEACON_INTERVAL_TU} TU",
            self.radio, self.ap_section, self.channel
        )
    }

    /// Step 3: the channels of the radio's PHY, filtered on the router to
    /// the frequency lines.
    fn channels_script(&self) -> Result<String> {
        Ok(format!(
            "phy=$(ubus call iwinfo phyname {} | jsonfilter -e '@.phyname') && test -n \"$phy\" \
             && info=$(iw phy \"$phy\" info) && printf '%s\\n' \"$info\" | grep -E \
             '^[[:space:]]*\\* [0-9]+(\\.[0-9]+)? MHz \\['",
            quote(&format!("{{\"section\":\"{}\"}}", self.radio))?
        ))
    }

    /// Step 4: the profile, committed and reloaded.
    fn apply_script(&self) -> Result<Zeroizing<String>> {
        let set = |section: &str, option: &str, value: &str| -> Result<String> {
            Ok(format!(
                "uci set {}",
                quote(&format!("wireless.{section}.{option}={value}"))?
            ))
        };
        let mut steps = Zeroizing::new(vec![
            set(&self.radio, "channel", &self.channel.to_string())?,
            set(&self.radio, "htmode", HTMODE)?,
            set(&self.radio, "beacon_int", &BEACON_INTERVAL_TU.to_string())?,
            set(&self.radio, "disabled", "0")?,
            set(&self.ap_section, "ssid", &self.ssid)?,
            set(&self.ap_section, "disabled", "0")?,
            set(&self.ap_section, "hidden", "0")?,
        ]);
        match &self.security {
            Security::Unchanged => {}
            Security::Wpa2(_) => {
                steps.push(set(&self.ap_section, "encryption", "psk2+ccmp")?);
                steps.push(set(&self.ap_section, "ieee80211r", "0")?);
                steps.push(set(&self.ap_section, "ieee80211w", "0")?);
                steps.push(set(
                    &self.ap_section,
                    "wpa_group_rekey",
                    &self.group_rekey_seconds.to_string(),
                )?);
            }
        }
        if self.station_network() {
            steps.push(set(&self.radio, "ieee80211ax", "0")?);
            steps.push(set(
                &self.ap_section,
                "network",
                crate::station::network::NAME,
            )?);
            steps.push(set(&self.ap_section, "isolate", "1")?);
        }
        steps.push("uci commit wireless".to_owned());
        if self.station_network() {
            // Recreate the selected AP: a hot reload can update the SSID while
            // retaining the previous bridge membership of its live netdev.
            steps.push(format!("wifi down {}", quote(&self.radio)?));
            steps.push(format!("wifi up {}", quote(&self.radio)?));
        } else {
            steps.push("wifi reload".to_owned());
        }
        let settings = Zeroizing::new(steps.join(" && "));
        if let Security::Wpa2(passphrase) = &self.security {
            let key = Zeroizing::new(quote(passphrase)?);
            // A heredoc feeds libuci's batch parser directly. Even on the
            // router, no process gets the key in argv. Printable ASCII
            // validation excludes newlines and delimiter injection.
            // UCI's cli.c::uci_batch returns 0 even on a command error;
            // require empty diagnostics as well, without reading back the key.
            Ok(Zeroizing::new(format!(
                "set +x\n{{\ndiagnostic=$(uci batch 2>&1 <<'OER_STATION_WPA2_KEY'\n\
                 set wireless.{}.key={}\nOER_STATION_WPA2_KEY\n)\nbatch_status=$?\n\
                 if test \"$batch_status\" -ne 0 || test -n \"$diagnostic\"; then printf '%s\\n' \"$diagnostic\" >&2; exit 1; fi\n\
                 {}\n}} >/dev/null",
                self.ap_section,
                key.as_str(),
                settings.as_str()
            )))
        } else {
            Ok(settings)
        }
    }

    /// Step 6: the AP interface, filtered on the router to the lines read.
    fn iw_script(&self) -> Result<String> {
        Ok(format!(
            "info=$(iw dev {} info) && printf '%s\\n' \"$info\" | grep -E \
             '^[[:space:]]*(addr|ssid|type|channel) '",
            quote(&self.interface)?
        ))
    }
}

/// The remote scripts in order, values as `<placeholders>`: what [`apply`]
/// runs, as the README lists them.
pub fn plan() -> Result<Vec<String>> {
    let profile = Profile {
        radio: "<radio>".to_owned(),
        ap_section: "<ap_section>".to_owned(),
        interface: "<wireless_interface>".to_owned(),
        channel: 0,
        ssid: Zeroizing::new("<ssid>".to_owned()),
        security: Security::Unchanged,
        group_rekey_seconds: crate::station_wpa2::GROUP_REKEY_SECONDS,
    };
    Ok(vec![
        CHANGES.to_owned(),
        show_script(),
        profile.channels_script()?,
        profile
            .apply_script()?
            .replace("channel=0", "channel=<channel>"),
        show_script(),
        profile.iw_script()?,
    ])
}

/// The applied profile: the stand AP's BSSID and channel, and the radio's
/// regulatory country.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Applied {
    pub bssid: Mac,
    pub channel: u8,
    /// The radio's UCI `country`: two uppercase letters.
    pub country: [u8; 2],
}

impl Applied {
    /// The one line that may be shown of it.
    pub fn describe(&self) -> String {
        format!("the stand AP {} is on channel {}", self.bssid, self.channel)
    }
}

/// Bring the router to `profile` and verify it; the stand AP's BSSID and
/// channel. Run twice, it changes nothing the second time and verifies the
/// same.
pub fn apply(remote: &mut dyn Remote, profile: &Profile) -> Result<Applied> {
    let result = apply_inner(remote, profile);
    if matches!(profile.security, Security::Wpa2(_)) {
        // A Remote implementation or a corrupt answer may echo its input.
        // No raw error from a credential-setting run crosses this boundary.
        result.map_err(|error| {
            let _hidden = Zeroizing::new(error);
            "router: applying or verifying the WPA2 station profile failed".to_owned()
        })
    } else {
        result
    }
}

fn apply_inner(remote: &mut dyn Remote, profile: &Profile) -> Result<Applied> {
    let changes = remote
        .run(CHANGES)
        .map_err(step("reading the staged changes"))?;
    refuse_staged_changes(&changes)?;
    let show = show_script();
    let before = Wireless::parse(
        &remote
            .run(&show)
            .map_err(step("reading the wireless configuration"))?,
        &profile.ssid,
    )?;
    let radio = before.check_before(profile)?;
    let channels = remote
        .run(&profile.channels_script()?)
        .map_err(step("reading the radio's channels"))?;
    check_channel(&channels, profile.channel, radio.option("country"))?;
    let previous_network = if profile.station_network() {
        let fingerprint = crate::station::network::prepare(remote)?;
        remote
            .run(&crate::station::network::render()?)
            .map_err(step("applying the isolated station network"))?;
        Some(fingerprint)
    } else {
        None
    };
    let _output = Zeroizing::new(
        remote
            .run(&profile.apply_script()?)
            .map_err(step("applying the profile"))?,
    );
    let after = Wireless::parse(
        &remote
            .run(&show)
            .map_err(step("reading the committed configuration"))?,
        &profile.ssid,
    )?;
    after.check_after(profile)?;
    let country = country_of(after.sections(profile)?.0)?;
    let applied = settle(remote, profile, country)?;
    if let Some(before) = previous_network {
        crate::station::network::verify(remote, &profile.interface, &before)?;
    }
    Ok(applied)
}

/// The two-letter `country` of a radio section.
fn country_of(radio: &Section) -> Result<[u8; 2]> {
    let country = radio.option("country").ok_or_else(|| {
        format!(
            "the router's radio {} has no country: the stand needs a regulatory domain",
            radio.name
        )
    })?;
    match country.as_bytes() {
        &[first, second] if first.is_ascii_uppercase() && second.is_ascii_uppercase() => {
            Ok([first, second])
        }
        _ => Err(format!(
            "the router's radio {} country is not two uppercase letters",
            radio.name
        )),
    }
}

/// An error of the step `what`.
fn step(what: &str) -> impl FnOnce(String) -> String + '_ {
    move |error| format!("router, {what}: {error}")
}

/// Step 6: the AP interface until it runs the profile.
fn settle(remote: &mut dyn Remote, profile: &Profile, country: [u8; 2]) -> Result<Applied> {
    let script = profile.iw_script()?;
    let mut last = String::new();
    for attempt in 0..SETTLE_ATTEMPTS {
        if attempt > 0 {
            remote.pause(SETTLE_PAUSE);
        }
        let info = match remote.run(&script) {
            Ok(text) => parse_iw_info(&text, &profile.ssid),
            Err(error) => Err(error),
        };
        match info {
            Ok(info) => match info.mismatch(profile.channel) {
                None => {
                    return Ok(Applied {
                        bssid: info.bssid,
                        channel: info.channel,
                        country,
                    });
                }
                Some(reason) => last = reason,
            },
            Err(error) => last = error,
        }
    }
    Err(format!(
        "router, reading the AP interface: it did not come up with the profile within {} s: \
         {last}",
        u64::from(SETTLE_ATTEMPTS - 1) * SETTLE_PAUSE.as_secs()
    ))
}

/// The staged changes as option paths, one per line (the router cut off
/// their values); none is an empty answer.
fn refuse_staged_changes(changes: &str) -> Result<()> {
    let staged = changes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|path| {
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

/// One `wireless` section, holding only its [`ALLOWED_OPTIONS`], its SSID
/// only as whether it is the stand's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Section {
    pub name: String,
    pub kind: String,
    pub options: Vec<(String, String)>,
    pub stand_ssid: bool,
    /// The options read so far, `ssid` included.
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

/// The wireless configuration, as the filtered `uci show wireless` prints
/// it.
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

/// A UCI section name as `uci show` prints it: neither `.` nor `=`.
fn shown_section(name: &str) -> bool {
    !name.is_empty() && !name.contains(['.', '='])
}

impl Wireless {
    /// `text`, the filtered `uci show wireless`; `ssid` is the stand's. A
    /// line outside the allowlist, which the router should have kept, is
    /// refused; a line is named by its number, never quoted.
    pub fn parse(text: &str, ssid: &str) -> Result<Self> {
        let mut wireless = Self::default();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let outside = || {
                format!(
                    "the wireless configuration's line {} is outside the allowlist (the \
                     router's filter let it through)",
                    number + 1
                )
            };
            let (path, value) = line.split_once('=').ok_or_else(outside)?;
            let path = path.strip_prefix("wireless.").ok_or_else(outside)?;
            match path.split_once('.') {
                None => {
                    if !shown_section(path) || !ALLOWED_TYPES.contains(&value) {
                        return Err(outside());
                    }
                    wireless.sections.push(Section {
                        name: path.to_owned(),
                        kind: value.to_owned(),
                        ..Section::default()
                    });
                }
                Some((name, option)) => {
                    if !shown_section(name) || !ALLOWED_OPTIONS.contains(&option) {
                        return Err(outside());
                    }
                    let mut items = uci_value(value).ok_or_else(|| {
                        format!(
                            "the wireless configuration's line {} holds no quoted value",
                            number + 1
                        )
                    })?;
                    // An allowed option of a section of another type, which
                    // the filter dropped.
                    let Some(section) = wireless
                        .sections
                        .iter_mut()
                        .rev()
                        .find(|section| section.name == name)
                    else {
                        continue;
                    };
                    if section.seen.iter().any(|seen| seen == option) {
                        return Err(format!(
                            "the wireless configuration's line {} repeats {name}.{option}",
                            number + 1
                        ));
                    }
                    section.seen.push(option.to_owned());
                    if option == "ssid" {
                        section.stand_ssid = items.as_slice() == [ssid];
                        continue;
                    }
                    if items.len() != 1 {
                        return Err(format!(
                            "the wireless configuration's {name}.{option} is a list, not one \
                             value"
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

    /// Step 2: the sections fit the profile; the radio.
    fn check_before(&self, profile: &Profile) -> Result<&Section> {
        let (radio, _) = self.sections(profile)?;
        self.refuse_others(profile)?;
        Ok(radio)
    }

    /// Step 5: the committed profile.
    fn check_after(&self, profile: &Profile) -> Result<()> {
        let (radio, ap) = self.sections(profile)?;
        let channel = profile.channel.to_string();
        let beacon_int = BEACON_INTERVAL_TU.to_string();
        for (section, option, wanted) in [
            (radio, "channel", channel.as_str()),
            (radio, "htmode", HTMODE),
            (radio, "beacon_int", beacon_int.as_str()),
            (radio, "disabled", "0"),
            (ap, "disabled", "0"),
            (ap, "hidden", "0"),
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
        if matches!(profile.security, Security::Wpa2(_))
            && (ap.option("encryption") != Some("psk2+ccmp")
                || ap.option("ieee80211r") != Some("0")
                || ap.option("ieee80211w") != Some("0")
                || ap.option("wpa_group_rekey")
                    != Some(profile.group_rekey_seconds.to_string().as_str())
                || radio.option("ieee80211ax") != Some("0"))
        {
            return Err(
                "the committed WPA2 station profile is not WPA2/CCMP with FT, PMF and HE disabled \
                 and the run's GTK rekey interval"
                    .to_owned(),
            );
        }
        self.refuse_others(profile)
    }
}

/// `* <MHz>[.0] MHz [<channel>] (…)` of `iw phy <phy> info`: the channel
/// and the rest of the line, which carries `disabled` or `no IR`.
fn phy_channel(line: &str) -> Option<(u8, u32, &str)> {
    let rest = line.trim().strip_prefix("* ")?;
    let (frequency, rest) = rest.split_once(" MHz [")?;
    let frequency = frequency
        .strip_suffix(".0")
        .unwrap_or(frequency)
        .parse::<u32>()
        .ok()?;
    let (channel, rest) = rest.split_once(']')?;
    Some((channel.parse().ok()?, frequency, rest))
}

/// Step 3: `channel` is offered by the radio's PHY (`text`, its frequency
/// lines) at 2.4 GHz and the regulatory domain allows an AP on it.
/// `country` is the radio's UCI `country`, for the message only.
pub fn check_channel(text: &str, channel: u8, country: Option<&str>) -> Result<()> {
    let domain = || {
        country.map_or_else(
            || "the radio's regulatory domain (no country set)".to_owned(),
            |country| format!("the radio's regulatory domain (country {country})"),
        )
    };
    let frequency = 2407 + 5 * u32::from(channel);
    let mut found = None;
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = phy_channel(line).ok_or_else(|| {
            format!(
                "the radio's channel list's line {} is outside the allowlist",
                number + 1
            )
        })?;
        if parsed.0 == channel && parsed.1 == frequency {
            found = Some(parsed.2);
        }
    }
    let flags = found.ok_or_else(|| format!("the radio does not offer channel {channel}"))?;
    if flags.contains("disabled") {
        return Err(format!("{} disables channel {channel}", domain()));
    }
    if flags.contains("no IR") {
        return Err(format!(
            "{} forbids initiating radiation (no IR) on channel {channel}: no AP there",
            domain()
        ));
    }
    Ok(())
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

/// What the filtered `iw dev <interface> info` printed; `ssid` is the
/// stand's. Only `addr`, `type`, `ssid` and `channel` lines are allowed;
/// nothing of the text is quoted in an error.
pub fn parse_iw_info(text: &str, ssid: &str) -> Result<IwInfo> {
    let mut bssid = None;
    let mut channel = None;
    let mut ap = false;
    let mut stand_ssid = false;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
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
            _ => {
                return Err(format!(
                    "iw: line {} is outside the allowlist (the router's filter let it through)",
                    number + 1
                ));
            }
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
mod tests;
