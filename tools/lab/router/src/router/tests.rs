use std::{io::Write as _, process::Stdio};

use super::*;

const SSID: &str = "Made Up 'Lab' Net";
/// The stand SSID as `uci show` quotes it.
const SSID_UCI: &str = r"Made Up '\''Lab'\'' Net";
const TARGET: &str = "made-up-router.lab";
const INTERFACE: &str = "phy7-ap3";
const BSSID: &str = "02:5e:c0:de:f6:d0";
const PASSPHRASE: &str = "made-up-passphrase";

fn profile() -> Profile {
    Profile::new("radio9", "lab_ap", INTERFACE, 6, SSID).unwrap()
}

/// What `uci show wireless` prints on a made-up router, secrets included.
fn raw_show(radio: &str, ap: &str) -> String {
    format!(
        "wireless.radio9=wifi-device\n\
         wireless.radio9.type='mac80211'\n\
         wireless.radio9.path='platform/soc/18000000.wifi'\n\
         wireless.radio9.band='2g'\n\
         wireless.radio9.country='DE'\n\
         wireless.radio9.cell_density='0'\n\
         {radio}\
         wireless.radio8=wifi-device\n\
         wireless.radio8.band='5g'\n\
         wireless.lab_ap=wifi-iface\n\
         wireless.lab_ap.device='radio9'\n\
         wireless.lab_ap.network='lan'\n\
         wireless.lab_ap.mode='ap'\n\
         wireless.lab_ap.encryption='psk2'\n\
         wireless.lab_ap.key='{PASSPHRASE}'\n\
         wireless.lab_ap.sae_password='made-up-sae'\n\
         wireless.lab_ap.auth_secret='made-up-radius'\n\
         wireless.lab_ap.wpa_psk_file='/etc/made-up-psk'\n\
         {ap}\
         wireless.five=wifi-iface\n\
         wireless.five.device='radio8'\n\
         wireless.five.mode='ap'\n\
         wireless.five.ssid='{SSID_UCI}'\n\
         wireless.five.key='made-up-five-key'\n\
         wireless.vlan1=wifi-vlan\n\
         wireless.vlan1.device='radio9'\n\
         wireless.vlan1.name='made-up-vlan'\n"
    )
}

/// The radio's and the AP section's profile options as `uci show` prints
/// them.
fn profiled(channel: &str) -> (String, String) {
    (
        format!(
            "wireless.radio9.channel='{channel}'\nwireless.radio9.htmode='HT20'\n\
             wireless.radio9.beacon_int='100'\nwireless.radio9.disabled='0'\n"
        ),
        format!(
            "wireless.lab_ap.ssid='{SSID_UCI}'\nwireless.lab_ap.disabled='0'\n\
             wireless.lab_ap.hidden='0'\n"
        ),
    )
}

/// `raw_show` as the router's allowlist filter passes it.
fn show(radio: &str, ap: &str) -> String {
    raw_show(radio, ap)
        .lines()
        .filter(|line| {
            let Some((path, value)) = line.split_once('=') else {
                return false;
            };
            match path.split('.').collect::<Vec<_>>().as_slice() {
                ["wireless", _] => ALLOWED_TYPES.contains(&value),
                ["wireless", _, option] => ALLOWED_OPTIONS.contains(option),
                _ => false,
            }
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

fn show_profiled() -> String {
    let (radio, ap) = profiled("6");
    show(&radio, &ap)
}

/// The router's state before the first W1 run: another channel, HE40, the
/// radio disabled, the AP hidden with another SSID.
fn show_before() -> String {
    show(
        "wireless.radio9.channel='1'\nwireless.radio9.htmode='HE40'\nwireless.radio9.disabled='1'\n",
        "wireless.lab_ap.ssid='Old Net'\nwireless.lab_ap.hidden='1'\n",
    )
}

/// `iw phy <phy> info`'s frequency lines.
const CHANNELS: &str = "\t\t\t* 2412.0 MHz [1] (20.0 dBm)\n\
                        \t\t\t* 2437.0 MHz [6] (20.0 dBm)\n\
                        \t\t\t* 2462.0 MHz [11] (20.0 dBm)\n\
                        \t\t\t* 2467.0 MHz [12] (20.0 dBm) (no IR)\n\
                        \t\t\t* 2472 MHz [13] (disabled)\n";

/// `iw dev <interface> info` in full.
fn raw_iw(channel: u8, width: &str, ssid: &str) -> String {
    let frequency = 2407 + 5 * u32::from(channel);
    format!(
        "Interface {INTERFACE}\n\
         \tifindex 12\n\
         \twdev 0x2\n\
         \taddr {BSSID}\n\
         \tssid {ssid}\n\
         \ttype AP\n\
         \twiphy 0\n\
         \tchannel {channel} ({frequency} MHz), width: {width}, center1: {frequency} MHz\n\
         \ttxpower 20.00 dBm\n"
    )
}

/// `raw_iw` as the router's filter passes it.
fn iw(channel: u8, width: &str, ssid: &str) -> String {
    raw_iw(channel, width, ssid)
        .lines()
        .filter(|line| {
            let word = line.trim().split(' ').next().unwrap_or_default();
            matches!(word, "addr" | "ssid" | "type" | "channel")
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Canned answers, matched by the script's start; records every script.
struct Canned {
    answers: Vec<(&'static str, Vec<Result<String>>)>,
    scripts: Vec<String>,
    pauses: u32,
}

impl Canned {
    fn new(answers: Vec<(&'static str, Vec<Result<String>>)>) -> Self {
        Self {
            answers,
            scripts: Vec::new(),
            pauses: 0,
        }
    }

    fn answer(&mut self, prefix: &str, answers: Vec<Result<String>>) {
        let (_, slot) = self
            .answers
            .iter_mut()
            .find(|(start, _)| *start == prefix)
            .expect("a canned prefix");
        *slot = answers;
    }
}

impl Remote for Canned {
    fn run(&mut self, script: &str) -> Result<String> {
        self.scripts.push(script.to_owned());
        let (_, answers) = self
            .answers
            .iter_mut()
            .find(|(prefix, _)| script.starts_with(prefix))
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

const CHANGES_PREFIX: &str = "changes=";
const SHOW_PREFIX: &str = "config=";
const CHANNELS_PREFIX: &str = "phy=";
const SET_PREFIX: &str = "uci set";
const IW_PREFIX: &str = "info=";

/// A router on its first W1 run: it settles after two reads.
fn first_run() -> Canned {
    Canned::new(vec![
        (CHANGES_PREFIX, vec![Ok("\n".to_owned())]),
        (SHOW_PREFIX, vec![Ok(show_before()), Ok(show_profiled())]),
        (CHANNELS_PREFIX, vec![Ok(CHANNELS.to_owned())]),
        (SET_PREFIX, vec![Ok(String::new())]),
        (
            IW_PREFIX,
            vec![
                Err("the remote command exited with 1".to_owned()),
                Ok(iw(1, "40 MHz", "Old\\x20Net")),
                Ok(iw(6, "20 MHz", SSID)),
            ],
        ),
    ])
}

/// A router already in the profile.
fn profiled_router() -> Canned {
    Canned::new(vec![
        (CHANGES_PREFIX, vec![Ok("\n".to_owned())]),
        (SHOW_PREFIX, vec![Ok(show_profiled())]),
        (CHANNELS_PREFIX, vec![Ok(CHANNELS.to_owned())]),
        (SET_PREFIX, vec![Ok(String::new())]),
        (IW_PREFIX, vec![Ok(iw(6, "20 MHz", SSID))]),
    ])
}

const APPLY: &str = "uci set 'wireless.radio9.channel=6' && uci set 'wireless.radio9.htmode=HT20' \
                     && uci set 'wireless.radio9.beacon_int=100' \
                     && uci set 'wireless.radio9.disabled=0' \
                     && uci set \"wireless.lab_ap.ssid=Made Up 'Lab' Net\" \
                     && uci set 'wireless.lab_ap.disabled=0' && uci set 'wireless.lab_ap.hidden=0' \
                     && uci commit wireless && wifi reload";

#[test]
fn the_profile_is_applied_verified_and_settles() {
    let mut remote = first_run();
    let applied = apply(&mut remote, &profile()).unwrap();
    assert_eq!(applied.bssid.octets(), [0x02, 0x5e, 0xc0, 0xde, 0xf6, 0xd0]);
    assert_eq!(applied.channel, 6);
    assert_eq!(applied.country, *b"DE");
    assert_eq!(remote.pauses, 2);
    let iw = profile().iw_script().unwrap();
    assert_eq!(
        remote.scripts,
        [
            CHANGES.to_owned(),
            show_script(),
            profile().channels_script().unwrap(),
            APPLY.to_owned(),
            show_script(),
            iw.clone(),
            iw.clone(),
            iw,
        ]
    );
}

#[test]
fn the_profile_enables_the_radio_and_shows_the_ssid() {
    // The router before: radio disabled, AP hidden; the profile sets both,
    // and the committed configuration must show them set.
    let mut remote = first_run();
    apply(&mut remote, &profile()).unwrap();
    let set = &remote.scripts[3];
    assert!(set.contains("'wireless.radio9.disabled=0'"), "{set}");
    assert!(set.contains("'wireless.lab_ap.hidden=0'"), "{set}");
    for (radio, ap, reason) in [
        ("wireless.radio9.disabled='1'\n", "", "radio9.disabled is 1"),
        ("", "wireless.lab_ap.hidden='1'\n", "lab_ap.hidden is 1"),
        ("", "wireless.lab_ap.disabled='1'\n", "lab_ap.disabled is 1"),
    ] {
        let (mut profiled_radio, mut profiled_ap) = profiled("6");
        if !radio.is_empty() {
            profiled_radio = profiled_radio.replace("wireless.radio9.disabled='0'\n", radio);
        }
        if !ap.is_empty() {
            let option = ap.split('=').next().unwrap();
            profiled_ap = profiled_ap
                .lines()
                .filter(|line| !line.starts_with(option))
                .map(|line| format!("{line}\n"))
                .collect::<String>()
                + ap;
        }
        let error = Wireless::parse(&show(&profiled_radio, &profiled_ap), SSID)
            .unwrap()
            .check_after(&profile())
            .unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn a_second_apply_changes_nothing_and_verifies_the_same() {
    let mut remote = profiled_router();
    let first = apply(&mut remote, &profile()).unwrap();
    let scripts = std::mem::take(&mut remote.scripts);
    let second = apply(&mut remote, &profile()).unwrap();
    assert_eq!(first, second);
    assert_eq!(scripts, remote.scripts);
    assert_eq!(remote.pauses, 0);
}

#[test]
fn quoting_survives_the_remote_shell() {
    for text in [SSID, "a b", "it's", "'", "$(reboot)", "a\\b", "`x`;rm"] {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", quote(text).unwrap()))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), text);
    }
}

#[test]
fn the_plan_lists_placeholders_only() {
    let plan = plan().unwrap();
    assert_eq!(plan.len(), 6);
    assert_eq!(
        plan[3],
        "uci set 'wireless.<radio>.channel=<channel>' && uci set 'wireless.<radio>.htmode=HT20' \
         && uci set 'wireless.<radio>.beacon_int=100' && uci set 'wireless.<radio>.disabled=0' \
         && uci set 'wireless.<ap_section>.ssid=<ssid>' && uci set 'wireless.<ap_section>.disabled=0' \
         && uci set 'wireless.<ap_section>.hidden=0' && uci commit wireless && wifi reload"
    );
    assert_eq!(
        plan[5],
        "info=$(iw dev '<wireless_interface>' info) && printf '%s\\n' \"$info\" | grep -E \
         '^[[:space:]]*(addr|ssid|type|channel) '"
    );
    // The README lists the very plan.
    let readme = include_str!("../../README.md");
    for script in &plan {
        assert!(readme.contains(script.as_str()), "README lacks `{script}`");
    }
}

/// Run `script` with a local `sh`, whose `uci`, `iw`, `ubus` and
/// `jsonfilter` print the made-up router's raw output: the router's own
/// filters, checked.
fn on_fake_router(script: &str, show: &str, changes: &str, iw_dev: &str, iw_phy: &str) -> String {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    let file = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    file("show", show);
    file("changes", changes);
    file("iw_dev", iw_dev);
    file("iw_phy", iw_phy);
    let tool = |name: &str, body: &str| {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    tool(
        "uci",
        "case \"$1 $2\" in 'show wireless') cat \"$FAKE/show\";; 'changes wireless') cat \"$FAKE/changes\";; *) exit 2;; esac",
    );
    tool(
        "iw",
        &format!(
            "case \"$1 $2 $3\" in 'dev {INTERFACE} info') cat \"$FAKE/iw_dev\";; 'phy phy0 info') cat \"$FAKE/iw_phy\";; *) exit 2;; esac"
        ),
    );
    tool(
        "ubus",
        "test \"$1 $2 $3 $4\" = 'call iwinfo phyname {\"section\":\"radio9\"}' && echo '{\"phyname\":\"phy0\"}'",
    );
    tool(
        "jsonfilter",
        "test \"$1 $2\" = '-e @.phyname' && cat >/dev/null && echo phy0",
    );
    let path = format!("{}:/usr/bin:/bin", dir.display());
    let mut child = std::process::Command::new("sh")
        .arg("-s")
        .env("PATH", path)
        .env("FAKE", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{script}");
    String::from_utf8(output.stdout).unwrap()
}

fn fake(script: &str) -> String {
    let (radio, ap) = profiled("6");
    on_fake_router(
        script,
        &raw_show(&radio, &ap),
        "wireless.lab_ap.key='made-up-staged-key'\nwireless.radio9.channel='11'\n",
        &raw_iw(6, "20 MHz", SSID),
        &format!(
            "Wiphy phy0\n\tmax # scan SSIDs: 4\n\tFrequencies:\n{CHANNELS}\tSupported commands:\n"
        ),
    )
}

#[test]
fn the_router_filters_by_the_allowlist() {
    let shown = fake(&show_script());
    for secret in [
        PASSPHRASE,
        "made-up-sae",
        "made-up-radius",
        "made-up-psk",
        "made-up-five-key",
        "made-up-vlan",
        "psk2",
        "mac80211",
        "'lan'",
    ] {
        assert!(!shown.contains(secret), "{secret}: {shown}");
    }
    assert_eq!(shown, show_profiled());
    let wireless = Wireless::parse(&shown, SSID).unwrap();
    wireless.check_after(&profile()).unwrap();
    assert_eq!(
        wireless.section("radio9").unwrap().option("country"),
        Some("DE")
    );
    // Staged changes leave the router as their paths.
    assert_eq!(
        fake(CHANGES),
        "wireless.lab_ap.key\nwireless.radio9.channel\n"
    );
    // The AP interface's four lines, the PHY's frequency lines.
    let iw_shown = fake(&profile().iw_script().unwrap());
    assert_eq!(iw_shown, iw(6, "20 MHz", SSID));
    assert!(!iw_shown.contains("txpower") && !iw_shown.contains("ifindex"));
    assert_eq!(fake(&profile().channels_script().unwrap()), CHANNELS);
}

#[test]
fn the_host_refuses_lines_outside_the_allowlist() {
    for line in [
        format!("wireless.lab_ap.key='{PASSPHRASE}'"),
        "wireless.lab_ap.encryption='psk2'".to_owned(),
        "wireless.vlan1=wifi-vlan".to_owned(),
        "network.lan=interface".to_owned(),
        "garbage".to_owned(),
        "wireless.a.b.c='x'".to_owned(),
    ] {
        let error = Wireless::parse(&format!("{}{line}\n", show_profiled()), SSID).unwrap_err();
        assert!(error.contains("outside the allowlist"), "{error}");
        assert!(
            !error.contains("made-up") && !error.contains("psk2"),
            "{error}"
        );
    }
    let error = parse_iw_info(
        &format!("{}\ttxpower 20.00 dBm\n", iw(6, "20 MHz", SSID)),
        SSID,
    )
    .unwrap_err();
    assert!(error.contains("outside the allowlist"), "{error}");
    let error = check_channel(&format!("{CHANNELS}Wiphy phy0\n"), 6, None).unwrap_err();
    assert!(error.contains("outside the allowlist"), "{error}");
}

#[test]
fn staged_changes_refuse_before_anything_is_set() {
    let mut remote = first_run();
    remote.answer(
        CHANGES_PREFIX,
        vec![Ok(
            "wireless.radio9.txpower\nwireless.lab_ap.key\n".to_owned()
        )],
    );
    let error = apply(&mut remote, &profile()).unwrap_err();
    assert!(
        error.contains("wireless.radio9.txpower, wireless.lab_ap.key"),
        "{error}"
    );
    assert_eq!(remote.scripts.len(), 1);
}

#[test]
fn the_regulatory_domain_refuses_before_anything_is_set() {
    check_channel(CHANNELS, 6, Some("DE")).unwrap();
    for (channel, reason) in [
        (
            12,
            "regulatory domain (country DE) forbids initiating radiation (no IR) on channel 12",
        ),
        (13, "regulatory domain (country DE) disables channel 13"),
        (2, "does not offer channel 2"),
    ] {
        let error = check_channel(CHANNELS, channel, Some("DE")).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
    // A channel number at another band's frequency is not the channel.
    assert!(check_channel("* 5180.0 MHz [6] (20.0 dBm)\n", 6, None).is_err());
    let error = check_channel(CHANNELS, 13, None).unwrap_err();
    assert!(error.contains("no country set"), "{error}");
    // Through apply: nothing is set.
    let mut remote = first_run();
    remote.answer(
        CHANNELS_PREFIX,
        vec![Ok(
            CHANNELS.replace("[6] (20.0 dBm)", "[6] (20.0 dBm) (no IR)")
        )],
    );
    let error = apply(&mut remote, &profile()).unwrap_err();
    assert!(error.contains("no IR"), "{error}");
    assert!(
        !remote
            .scripts
            .iter()
            .any(|script| script.starts_with(SET_PREFIX)),
        "{:?}",
        remote.scripts
    );
}

#[test]
fn uci_values_are_parsed_and_only_whether_an_ssid_is_the_stands_kept() {
    let wireless = Wireless::parse(&show_profiled(), SSID).unwrap();
    let debug = format!("{wireless:?}");
    for secret in ["Made Up", "Lab'", PASSPHRASE] {
        assert!(!debug.contains(secret), "{secret}: {debug}");
    }
    let ap = wireless.section("lab_ap").unwrap();
    assert!(ap.stand_ssid);
    assert_eq!(ap.option("mode"), Some("ap"));
    // The quote-escaped SSID of the 5 GHz section is the stand's too.
    assert!(wireless.section("five").unwrap().stand_ssid);
    // The allowed options of a dropped section type are ignored.
    assert!(wireless.section("vlan1").is_none());
    assert!(
        !Wireless::parse(&show_before(), SSID)
            .unwrap()
            .section("lab_ap")
            .unwrap()
            .stand_ssid
    );
}

#[test]
fn malformed_uci_values_are_named_by_line_only() {
    for (text, reason) in [
        (
            "wireless.radio9=wifi-device\nwireless.radio9.channel=6\n",
            "line 2 holds no quoted value",
        ),
        (
            "wireless.radio9=wifi-device\nwireless.radio9.channel='6\n",
            "line 2 holds no quoted value",
        ),
        (
            "wireless.radio9=wifi-device\nwireless.radio9.channel='6'\nwireless.radio9.channel='7'\n",
            "line 3 repeats radio9.channel",
        ),
        (
            "wireless.radio9=wifi-device\nwireless.radio9.channel='1' '6'\n",
            "is a list",
        ),
    ] {
        let error = Wireless::parse(text, SSID).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn the_sections_must_fit_before_anything_is_set() {
    let profile = profile();
    let check = |text: String| {
        Wireless::parse(&text, SSID)
            .unwrap()
            .check_before(&profile)
            .map(|_| ())
    };
    // Disabled and hidden are the profile's to fix.
    check(show_before()).unwrap();
    let error = check(show(
        "",
        "wireless.guest=wifi-iface\nwireless.guest.device='radio9'\nwireless.guest.mode='ap'\n\
         wireless.guest.ssid='Made Up '\\''Lab'\\'' Net'\n",
    ))
    .unwrap_err();
    assert!(error.contains("carry the stand's SSID: guest;"), "{error}");
    assert!(!error.contains("Made Up"), "{error}");
    // A disabled duplicate is no second stand.
    check(show(
        "",
        "wireless.guest=wifi-iface\nwireless.guest.device='radio9'\n\
         wireless.guest.ssid='Made Up '\\''Lab'\\'' Net'\nwireless.guest.disabled='1'\n",
    ))
    .unwrap();
    for (from, to, reason) in [
        (
            "lab_ap.device='radio9'",
            "lab_ap.device='radio8'",
            "belongs to another radio",
        ),
        ("lab_ap.mode='ap'", "lab_ap.mode='sta'", "is not in mode ap"),
        (
            "radio9.band='2g'",
            "radio9.band='5g'",
            "not a 2.4 GHz radio",
        ),
        (
            "wireless.lab_ap=wifi-iface",
            "wireless.lab_ap=wifi-device",
            "no wifi-iface section lab_ap",
        ),
    ] {
        let error = check(show_before().replace(from, to)).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn the_committed_profile_is_checked() {
    let check = |radio: &str, ap: &str| {
        Wireless::parse(&show(radio, ap), SSID)
            .unwrap()
            .check_after(&profile())
    };
    let (radio, ap) = profiled("6");
    check(&radio, &ap).unwrap();
    for (radio, ap, reason) in [
        (
            profiled("11").0,
            ap.clone(),
            "radio9.channel is 11, not the profile's 6",
        ),
        (
            radio.replace("HT20", "HE20"),
            ap.clone(),
            "radio9.htmode is HE20",
        ),
        (
            radio.clone(),
            ap.replace(SSID_UCI, "Other"),
            "lab_ap.ssid is not the stand's SSID",
        ),
    ] {
        let error = check(&radio, &ap).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn iw_info_parses_and_refuses_garbage() {
    let info = parse_iw_info(&iw(6, "20 MHz", SSID), SSID).unwrap();
    assert_eq!(info.bssid.octets(), [0x02, 0x5e, 0xc0, 0xde, 0xf6, 0xd0]);
    assert_eq!((info.channel, info.width, info.ap), (6, Width::Ht20, true));
    assert!(info.stand_ssid);
    assert_eq!(info.mismatch(6), None);
    assert_eq!(
        parse_iw_info(&iw(6, "20 MHz (no HT)", SSID), SSID)
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
    let good = iw(6, "20 MHz", SSID);
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
            "outside the allowlist",
        ),
        ("\u{0}\u{1}garbage\n".to_owned(), "outside the allowlist"),
        (String::new(), "no addr line"),
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
    let mut remote = first_run();
    remote.answer(IW_PREFIX, vec![Ok(iw(1, "20 MHz", SSID))]);
    let error = apply(&mut remote, &profile()).unwrap_err();
    assert!(error.contains("within 30 s"), "{error}");
    assert!(error.contains("channel 1, the profile's is 6"), "{error}");
    assert_eq!(remote.pauses, SETTLE_ATTEMPTS - 1);
}

#[test]
fn a_failing_ssh_is_an_error_without_the_target() {
    let mut remote = Canned::new(vec![(
        "",
        vec![Err(
            "ssh could not reach the router or was refused (exit 255; BatchMode needs a loaded key)"
                .to_owned(),
        )],
    )]);
    let error = apply(&mut remote, &profile()).unwrap_err();
    assert!(
        error.starts_with("router, reading the staged changes: ssh could not"),
        "{error}"
    );
    assert_eq!(remote.scripts.len(), 1);
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
    let applied = apply(&mut first_run(), &profile).unwrap();
    let ssh = crate::ssh::Ssh::new(TARGET).unwrap();
    let info = parse_iw_info(&iw(6, "20 MHz", SSID), SSID).unwrap();
    let wireless = Wireless::parse(&show_profiled(), SSID).unwrap();
    let shown = [
        format!("{profile:?}"),
        profile.describe(),
        format!("{applied:?}"),
        applied.describe(),
        format!("{ssh:?}"),
        format!("{info:?}"),
        format!("{wireless:?}"),
        plan().unwrap().join("\n"),
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
        PASSPHRASE,
    ] {
        assert!(!shown.contains(secret), "{secret}: {shown}");
    }
    assert!(shown.contains("xx:xx:xx:xx:F6:D0"), "{shown}");
}

#[test]
fn the_radio_country_is_read_without_changing_anything() {
    let mut remote = Canned::new(vec![(SHOW_PREFIX, vec![Ok(show_before())])]);
    assert_eq!(radio_country(&mut remote, "radio9").unwrap(), *b"DE");
    assert_eq!(remote.scripts, [show_script()]);
    let mut remote = Canned::new(vec![(SHOW_PREFIX, vec![Ok(show_before())])]);
    assert!(radio_country(&mut remote, "radio7").is_err());
}

#[test]
fn a_radio_without_a_valid_country_is_refused() {
    for replacement in [
        "",
        "wireless.radio9.country='de'\n",
        "wireless.radio9.country='DEU'\n",
    ] {
        let text = show_before().replace("wireless.radio9.country='DE'\n", replacement);
        let mut remote = Canned::new(vec![(SHOW_PREFIX, vec![Ok(text)])]);
        let error = radio_country(&mut remote, "radio9").unwrap_err();
        assert!(error.contains("country"), "{error}");
    }
}

#[test]
fn shell_quote_rejects_nul_and_preserves_other_words() {
    assert!(quote("nul\0byte").unwrap_err().contains("nul"));
    for value in [
        "",
        "plain",
        "line\nbreak",
        "two words",
        "a'\"b",
        "$(exit 9); #",
        "ä\\b",
    ] {
        let quoted = quote(value).unwrap();
        let words = shlex::split(&quoted).unwrap();
        assert_eq!(words, [value]);
    }
}
