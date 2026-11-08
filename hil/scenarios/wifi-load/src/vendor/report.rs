//! The peer's `REPORT` of one run, decoded and checked before any of it is
//! measured. Every line the reply holds must be one the protocol names for
//! the run (`@RX`, `@SPAN`, `@LWIP` and `@WIFIRX` of the window and of the
//! run, `@HWRX` of the window, `@LINK`), each once, each naming the run, each
//! with exactly its fields; a count is an unsigned number, a time `-1` or
//! unsigned. The values must agree with the run the host asked for (its
//! window, delay and drain, the joined channel) and with each other (the
//! tally's own arithmetic, a window within its run, a part within its
//! whole). Anything else is an error: a report is measured whole or not at
//! all.

use std::collections::BTreeMap;

use oer_tools_lab_model::peer_line;
use serde_json::{Value, json};

/// The peer's tally covers sequences `0..TALLY_CAPACITY` (its 4096 words of
/// 64 bits); a higher one counts as untracked.
pub const TALLY_CAPACITY: u64 = 1 << 18;
/// The load header's length: no load datagram is shorter.
const HEADER: u64 = 28;
/// The longest UDP payload.
const MAX_PAYLOAD: u64 = 65_507;
/// How late the peer's run task may take a window or run boundary: its
/// scheduler tick is 10 ms, and an lwIP sample precedes each snapshot.
const BOUNDARY_SLACK_US: u64 = 50_000;
/// How long after the drain the peer may take to confirm its counter
/// stopped (its `STOP_CONFIRM_MS`).
const STOP_CONFIRM_US: u64 = 2_000_000;

/// The run the host asked for, which the report must describe.
#[derive(Clone, Copy, Debug)]
pub struct Expect {
    pub run: u32,
    pub delay_ms: u32,
    pub window_ms: u32,
    pub drain_ms: u32,
    /// The channel the peer joined.
    pub channel: u8,
    /// The disconnections the peer counted when it joined: a joined link
    /// that stayed up counts no more.
    pub disconnects: u64,
}

/// `@RX`: the tally of the run's load datagrams.
#[derive(Clone, Debug, PartialEq)]
pub struct Rx {
    pub datagrams: u64,
    pub bytes: u64,
    pub window_datagrams: u64,
    pub window_bytes: u64,
    pub lost: u64,
    pub reordered: u64,
    pub duplicates: u64,
    pub untracked: u64,
    pub highest: Option<u64>,
    pub foreign: u64,
    pub echoed: u64,
    pub echo_refused: u64,
    pub receive_errors: u64,
    pub first_us: Option<u64>,
    pub last_us: Option<u64>,
    pub window_us: u64,
}

/// `@SPAN`: when the peer took its window's and run's boundaries, from its
/// RX command.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub window_from_us: u64,
    pub window_to_us: u64,
    pub run_to_us: u64,
}

/// `@LWIP`: the IP stack's counters over a span.
#[derive(Clone, Debug, PartialEq)]
pub struct Lwip {
    pub link_recv: u64,
    pub link_drop: u64,
    pub ip_recv: u64,
    pub ip_drop: u64,
    pub udp_recv: u64,
    pub udp_drop: u64,
    pub udp_memerr: u64,
}

/// `@WIFIRX`: the driver's TID 0 reception statistics over a span.
#[derive(Clone, Debug, PartialEq)]
pub struct WifiRx {
    pub legacy: u64,
    pub legacy_noeb: u64,
    pub ht: u64,
    pub ht_noeb: u64,
    pub ht_stbc: u64,
    pub ht_retry: u64,
    pub su: u64,
    pub su_retry: u64,
    pub rx_isr: u64,
    pub rx_nblks: u64,
    pub rx_reset_rxbase_cnt: u64,
    pub rx_base_null_cnt: u64,
}

/// `@LINK`: the joined link at the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub channel: u64,
    pub second: u64,
    pub rssi: i64,
    pub disconnects: u64,
}

/// A whole, consistent report of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub rx: Rx,
    pub span: Span,
    pub lwip_window: Lwip,
    pub lwip_run: Lwip,
    pub wifi_rx_window: WifiRx,
    pub wifi_rx_run: WifiRx,
    /// The MAC's receive counter registers over the window, by name.
    pub hw_rx: BTreeMap<&'static str, i64>,
    pub link: Link,
}

const RX_KEYS: [&str; 16] = [
    "datagrams",
    "bytes",
    "window_datagrams",
    "window_bytes",
    "lost",
    "reordered",
    "duplicates",
    "untracked",
    "highest",
    "foreign",
    "echoed",
    "echo_refused",
    "receive_errors",
    "first_us",
    "last_us",
    "window_us",
];
const SPAN_KEYS: [&str; 3] = ["window_from_us", "window_to_us", "run_to_us"];
const LWIP_KEYS: [&str; 7] = [
    "link_recv",
    "link_drop",
    "ip_recv",
    "ip_drop",
    "udp_recv",
    "udp_drop",
    "udp_memerr",
];
const WIFI_RX_KEYS: [&str; 13] = [
    "tid",
    "legacy",
    "legacy_noeb",
    "ht",
    "ht_noeb",
    "ht_stbc",
    "ht_retry",
    "su",
    "su_retry",
    "rx_isr",
    "rx_nblks",
    "rx_reset_rxbase_cnt",
    "rx_base_null_cnt",
];
/// The MAC's counters as `esp_test_hw_rx_statistics_t` holds them: each a
/// 16-bit field but the two 32-bit ones and the signed frequency offset.
const HW_RX_U16: [&str; 32] = [
    "rx_fcs_err",
    "rx_abort",
    "rx_abort_fcs_pass",
    "nrx_err_pwrdrop",
    "rx_samebm_errcnt",
    "rx_mpdu",
    "rx_end_cnt",
    "rx_datasuc",
    "rx_sf",
    "rx_other_ucast",
    "rx_buf_fullcnt",
    "rx_fifo_ovfcnt",
    "rx_tkip_errcnt",
    "rx_btblock_err",
    "rx_freqhop_err",
    "rx_lastunmatch_err",
    "rx_ack_int_cnt",
    "rx_rts_int_cnt",
    "brx_err_agc",
    "brx_err",
    "nrx_err",
    "nrx_err_abort",
    "nrx_err_agcexit",
    "nrx_err_bboff",
    "nrx_err_fdm_wdg",
    "nrx_err_restart",
    "nrx_err_serv",
    "nrx_err_txover",
    "nrx_err_unsupport",
    "nrx_htsig_err",
    "rxhung_statis",
    "txhung_statis",
];
const HW_RX_U32: [&str; 2] = ["rxtxhung", "rxtxpanic"];
const HW_RX_SIGNED: &str = "rx_cfo_hz";
const LINK_KEYS: [&str; 5] = ["state", "channel", "second", "rssi", "disconnects"];

/// One line's fields: each key once, exactly the line's own.
struct Fields<'a> {
    what: String,
    values: BTreeMap<&'a str, &'a str>,
}

impl<'a> Fields<'a> {
    /// `report`'s fields, which must be exactly `run`, `span` when the line
    /// has one, and `keys`.
    fn of(
        report: &'a peer_line::Report,
        span: Option<&str>,
        keys: &[&str],
    ) -> Result<Self, String> {
        let what = span.map_or_else(
            || format!("@{}", report.name),
            |span| format!("@{} span={span}", report.name),
        );
        let mut values = BTreeMap::new();
        for field in &report.fields {
            let (key, value) = field
                .split_once('=')
                .ok_or_else(|| format!("{what}: `{field}` is no key=value field"))?;
            if values.insert(key, value).is_some() {
                return Err(format!("{what}: `{key}` appears twice"));
            }
        }
        let mut expected = keys.to_vec();
        expected.push("run");
        if span.is_some() {
            expected.push("span");
        }
        if let Some(extra) = values.keys().find(|key| !expected.contains(key)) {
            return Err(format!("{what}: an unexpected field `{extra}`"));
        }
        if let Some(missing) = expected.iter().find(|key| !values.contains_key(*key)) {
            return Err(format!("{what}: no `{missing}`"));
        }
        Ok(Self { what, values })
    }

    fn text(&self, key: &str) -> Result<&'a str, String> {
        self.values
            .get(key)
            .copied()
            .ok_or_else(|| format!("{}: no `{key}`", self.what))
    }

    fn unsigned(&self, key: &str) -> Result<u64, String> {
        let text = self.text(key)?;
        text.parse()
            .map_err(|_| format!("{}: `{key}={text}` is no unsigned count", self.what))
    }

    fn signed(&self, key: &str) -> Result<i64, String> {
        let text = self.text(key)?;
        text.parse()
            .map_err(|_| format!("{}: `{key}={text}` is no number", self.what))
    }

    /// `-1` for none, else an unsigned value.
    fn optional(&self, key: &str) -> Result<Option<u64>, String> {
        match self.text(key)? {
            "-1" => Ok(None),
            _ => self.unsigned(key).map(Some),
        }
    }

    fn check(&self, holds: bool, what: impl std::fmt::Display) -> Result<(), String> {
        if holds {
            Ok(())
        } else {
            Err(format!("{}: {what}", self.what))
        }
    }
}

/// The lines the protocol names for a run, by name and span.
const LINES: [(&str, Option<&str>); 8] = [
    ("RX", None),
    ("SPAN", None),
    ("LWIP", Some("window")),
    ("LWIP", Some("run")),
    ("WIFIRX", Some("window")),
    ("WIFIRX", Some("run")),
    ("HWRX", Some("window")),
    ("LINK", None),
];

/// Each of [`LINES`] among `reports`, once; any other line, a second one,
/// or one of another run is an error.
fn lines(
    reports: &[peer_line::Report],
    run: u32,
) -> Result<[&peer_line::Report; LINES.len()], String> {
    let mut found: [Option<&peer_line::Report>; LINES.len()] = [None; LINES.len()];
    for report in reports {
        let span = report.field("span");
        let what = format!(
            "@{}{}",
            report.name,
            span.map_or_else(String::new, |span| format!(" span={span}"))
        );
        let index = LINES
            .iter()
            .position(|(name, expected)| *name == report.name && *expected == span)
            .ok_or_else(|| format!("the report holds an unexpected {what} line"))?;
        let of = report.field("run").unwrap_or_default();
        if of != run.to_string() {
            return Err(format!("{what} is of run `{of}`, not {run}"));
        }
        let slot = found
            .get_mut(index)
            .ok_or_else(|| format!("{what}: no slot"))?;
        if slot.replace(report).is_some() {
            return Err(format!("the report holds {what} twice"));
        }
    }
    let mut lines = Vec::with_capacity(LINES.len());
    for ((name, span), report) in LINES.iter().zip(found) {
        lines.push(report.ok_or_else(|| {
            format!(
                "the report holds no @{name}{} line",
                span.map_or_else(String::new, |span| format!(" span={span}"))
            )
        })?);
    }
    lines
        .try_into()
        .map_err(|_| "the report's lines".to_owned())
}

fn rx(report: &peer_line::Report, expect: &Expect) -> Result<Rx, String> {
    let f = Fields::of(report, None, &RX_KEYS)?;
    let rx = Rx {
        datagrams: f.unsigned("datagrams")?,
        bytes: f.unsigned("bytes")?,
        window_datagrams: f.unsigned("window_datagrams")?,
        window_bytes: f.unsigned("window_bytes")?,
        lost: f.unsigned("lost")?,
        reordered: f.unsigned("reordered")?,
        duplicates: f.unsigned("duplicates")?,
        untracked: f.unsigned("untracked")?,
        highest: f.optional("highest")?,
        foreign: f.unsigned("foreign")?,
        echoed: f.unsigned("echoed")?,
        echo_refused: f.unsigned("echo_refused")?,
        receive_errors: f.unsigned("receive_errors")?,
        first_us: f.optional("first_us")?,
        last_us: f.optional("last_us")?,
        window_us: f.unsigned("window_us")?,
    };
    let window_us = u64::from(expect.window_ms) * 1000;
    f.check(
        rx.window_us == window_us,
        format!("window_us={} for a window of {window_us} us", rx.window_us),
    )?;
    f.check(
        rx.window_datagrams <= rx.datagrams && rx.window_bytes <= rx.bytes,
        "the window holds more than the run",
    )?;
    let lengths = |datagrams: u64, bytes: u64| {
        datagrams
            .checked_mul(HEADER)
            .is_some_and(|least| least <= bytes)
            && datagrams
                .checked_mul(MAX_PAYLOAD)
                .is_some_and(|most| bytes <= most)
    };
    f.check(
        lengths(rx.datagrams, rx.bytes) && lengths(rx.window_datagrams, rx.window_bytes),
        "bytes outside the datagrams' possible lengths",
    )?;
    match (rx.highest, rx.first_us, rx.last_us) {
        (None, None, None) => f.check(
            rx.datagrams == 0 && rx.lost == 0 && rx.reordered == 0,
            "counts without a highest sequence",
        )?,
        (Some(highest), Some(first), Some(last)) => {
            f.check(
                highest < TALLY_CAPACITY,
                "a highest sequence past the tally",
            )?;
            let below = highest + 1;
            f.check(
                rx.datagrams >= 1 && rx.datagrams <= below,
                "more distinct datagrams than sequences up to the highest",
            )?;
            f.check(
                rx.lost == below - rx.datagrams,
                format!("lost={} for {} of {below} sequences", rx.lost, rx.datagrams),
            )?;
            f.check(
                rx.reordered < rx.datagrams,
                "more reordered than distinct datagrams",
            )?;
            f.check(first <= last, "the first datagram after the last")?;
        }
        _ => {
            return Err(format!(
                "{}: highest, first_us and last_us disagree",
                f.what
            ));
        }
    }
    Ok(rx)
}

fn span(report: &peer_line::Report, expect: &Expect) -> Result<Span, String> {
    let f = Fields::of(report, None, &SPAN_KEYS)?;
    let span = Span {
        window_from_us: f.unsigned("window_from_us")?,
        window_to_us: f.unsigned("window_to_us")?,
        run_to_us: f.unsigned("run_to_us")?,
    };
    let us = |ms: u32| u64::from(ms) * 1000;
    let from = us(expect.delay_ms);
    let to = from + us(expect.window_ms);
    let end = to + us(expect.drain_ms);
    let within = |at: u64, target: u64, late: u64| at >= target && at <= target + late;
    f.check(
        within(span.window_from_us, from, BOUNDARY_SLACK_US),
        format!(
            "the window opened at {} us, due at {from}",
            span.window_from_us
        ),
    )?;
    f.check(
        within(span.window_to_us, to, BOUNDARY_SLACK_US),
        format!("the window closed at {} us, due at {to}", span.window_to_us),
    )?;
    f.check(
        within(span.run_to_us, end, STOP_CONFIRM_US + BOUNDARY_SLACK_US),
        format!("the run ended at {} us, due at {end}", span.run_to_us),
    )?;
    Ok(span)
}

fn lwip(report: &peer_line::Report, span: &str) -> Result<Lwip, String> {
    let f = Fields::of(report, Some(span), &LWIP_KEYS)?;
    let lwip = Lwip {
        link_recv: f.unsigned("link_recv")?,
        link_drop: f.unsigned("link_drop")?,
        ip_recv: f.unsigned("ip_recv")?,
        ip_drop: f.unsigned("ip_drop")?,
        udp_recv: f.unsigned("udp_recv")?,
        udp_drop: f.unsigned("udp_drop")?,
        udp_memerr: f.unsigned("udp_memerr")?,
    };
    f.check(
        lwip.udp_recv <= lwip.ip_recv,
        "more UDP datagrams than IP packets",
    )?;
    Ok(lwip)
}

fn wifi_rx(report: &peer_line::Report, span: &str) -> Result<WifiRx, String> {
    let f = Fields::of(report, Some(span), &WIFI_RX_KEYS)?;
    f.check(f.unsigned("tid")? == 0, "not TID 0")?;
    let rx = WifiRx {
        legacy: f.unsigned("legacy")?,
        legacy_noeb: f.unsigned("legacy_noeb")?,
        ht: f.unsigned("ht")?,
        ht_noeb: f.unsigned("ht_noeb")?,
        ht_stbc: f.unsigned("ht_stbc")?,
        ht_retry: f.unsigned("ht_retry")?,
        su: f.unsigned("su")?,
        su_retry: f.unsigned("su_retry")?,
        rx_isr: f.unsigned("rx_isr")?,
        rx_nblks: f.unsigned("rx_nblks")?,
        rx_reset_rxbase_cnt: f.unsigned("rx_reset_rxbase_cnt")?,
        rx_base_null_cnt: f.unsigned("rx_base_null_cnt")?,
    };
    f.check(
        rx.ht_noeb <= rx.ht && rx.ht_retry <= rx.ht && rx.legacy_noeb <= rx.legacy,
        "a part larger than its whole",
    )?;
    Ok(rx)
}

fn hw_rx(report: &peer_line::Report) -> Result<BTreeMap<&'static str, i64>, String> {
    let mut keys = HW_RX_U16.to_vec();
    keys.extend(HW_RX_U32);
    keys.push(HW_RX_SIGNED);
    let f = Fields::of(report, Some("window"), &keys)?;
    let mut values = BTreeMap::new();
    for key in HW_RX_U16 {
        let value = f.unsigned(key)?;
        f.check(
            value <= u64::from(u16::MAX),
            format!("`{key}` past its 16 bits"),
        )?;
        values.insert(key, i64::from(value as u16));
    }
    for key in HW_RX_U32 {
        let value = f.unsigned(key)?;
        f.check(
            value <= u64::from(u32::MAX),
            format!("`{key}` past its 32 bits"),
        )?;
        values.insert(key, i64::from(value as u32));
    }
    let offset = f.signed(HW_RX_SIGNED)?;
    f.check(
        i64::from(i16::MIN) <= offset && offset <= i64::from(i16::MAX),
        format!("`{HW_RX_SIGNED}` past its 16 bits"),
    )?;
    values.insert(HW_RX_SIGNED, offset);
    Ok(values)
}

fn link(report: &peer_line::Report, expect: &Expect) -> Result<Link, String> {
    if report.field("state") != Some("up") {
        return Err(format!(
            "@LINK: the link is not up at the report ({})",
            report.fields.join(" ")
        ));
    }
    let f = Fields::of(report, None, &LINK_KEYS)?;
    let link = Link {
        channel: f.unsigned("channel")?,
        second: f.unsigned("second")?,
        rssi: f.signed("rssi")?,
        disconnects: f.unsigned("disconnects")?,
    };
    f.check(
        link.channel == u64::from(expect.channel),
        format!("channel {} after joining {}", link.channel, expect.channel),
    )?;
    f.check(
        link.second <= 2,
        format!("secondary channel code {}", link.second),
    )?;
    f.check(
        (-128..=0).contains(&link.rssi),
        format!("rssi {}", link.rssi),
    )?;
    f.check(
        link.disconnects == expect.disconnects,
        format!(
            "{} disconnections, {} at the join",
            link.disconnects, expect.disconnects
        ),
    )?;
    Ok(link)
}

/// `@JOINED`: the link the peer joined.
#[derive(Clone, Debug, PartialEq)]
pub struct Joined {
    pub channel: u8,
    pub second: u64,
    pub rssi: i64,
    pub phymode: u64,
    pub address: std::net::Ipv4Addr,
    pub disconnects: u64,
}

const JOINED_KEYS: [&str; 6] = ["channel", "second", "rssi", "phymode", "ip", "disconnects"];

/// The one `@JOINED` line among `reports`, on `channel`.
pub fn joined(reports: &[peer_line::Report], channel: u8) -> Result<Joined, String> {
    let mut found = reports.iter().filter(|report| report.name == "JOINED");
    let report = found
        .next()
        .ok_or("the peer joined without its @JOINED line")?;
    if found.next().is_some() {
        return Err("the peer reported @JOINED twice".to_owned());
    }
    let mut keys = JOINED_KEYS.to_vec();
    // `run` belongs to a run's lines; a join has none.
    let mut fields = report.clone();
    fields.fields.push("run=0".to_owned());
    keys.sort_unstable();
    let f = Fields::of(&fields, None, &keys)?;
    let text = f.text("ip")?;
    let joined = Joined {
        channel: u8::try_from(f.unsigned("channel")?)
            .map_err(|_| "@JOINED: channel past 255".to_owned())?,
        second: f.unsigned("second")?,
        rssi: f.signed("rssi")?,
        phymode: f.unsigned("phymode")?,
        address: text
            .parse()
            .map_err(|_| format!("@JOINED: `ip={text}` is no IPv4 address"))?,
        disconnects: f.unsigned("disconnects")?,
    };
    f.check(
        joined.channel == channel,
        format!("joined channel {} for {channel}", joined.channel),
    )?;
    f.check(
        joined.second <= 2,
        format!("secondary channel code {}", joined.second),
    )?;
    f.check(
        (-128..=0).contains(&joined.rssi),
        format!("rssi {}", joined.rssi),
    )?;
    f.check(!joined.address.is_unspecified(), "the unspecified address")?;
    Ok(joined)
}

/// Every counter of `window` at most `run`'s.
fn within<T>(
    window: &T,
    run: &T,
    counters: fn(&T) -> Vec<(&'static str, u64)>,
) -> Result<(), String> {
    for ((name, inner), (_, outer)) in counters(window).into_iter().zip(counters(run)) {
        if inner > outer {
            return Err(format!(
                "`{name}` counts {inner} in the window, {outer} in its run"
            ));
        }
    }
    Ok(())
}

fn lwip_counters(lwip: &Lwip) -> Vec<(&'static str, u64)> {
    vec![
        ("link_recv", lwip.link_recv),
        ("link_drop", lwip.link_drop),
        ("ip_recv", lwip.ip_recv),
        ("ip_drop", lwip.ip_drop),
        ("udp_recv", lwip.udp_recv),
        ("udp_drop", lwip.udp_drop),
        ("udp_memerr", lwip.udp_memerr),
    ]
}

fn wifi_rx_counters(rx: &WifiRx) -> Vec<(&'static str, u64)> {
    vec![
        ("legacy", rx.legacy),
        ("legacy_noeb", rx.legacy_noeb),
        ("ht", rx.ht),
        ("ht_noeb", rx.ht_noeb),
        ("ht_stbc", rx.ht_stbc),
        ("ht_retry", rx.ht_retry),
        ("su", rx.su),
        ("su_retry", rx.su_retry),
        ("rx_isr", rx.rx_isr),
        ("rx_nblks", rx.rx_nblks),
        ("rx_reset_rxbase_cnt", rx.rx_reset_rxbase_cnt),
        ("rx_base_null_cnt", rx.rx_base_null_cnt),
    ]
}

/// Decode `reports`, the lines of the peer's reply to `REPORT`, as the
/// report of `expect`'s run.
pub fn decode(reports: &[peer_line::Report], expect: &Expect) -> Result<Report, String> {
    let [
        rx_line,
        span_line,
        lwip_window,
        lwip_run,
        wifi_window,
        wifi_run,
        hw_line,
        link_line,
    ] = lines(reports, expect.run)?;
    let report = Report {
        rx: rx(rx_line, expect)?,
        span: span(span_line, expect)?,
        lwip_window: lwip(lwip_window, "window")?,
        lwip_run: lwip(lwip_run, "run")?,
        wifi_rx_window: wifi_rx(wifi_window, "window")?,
        wifi_rx_run: wifi_rx(wifi_run, "run")?,
        hw_rx: hw_rx(hw_line)?,
        link: link(link_line, expect)?,
    };
    within(&report.lwip_window, &report.lwip_run, lwip_counters)?;
    within(
        &report.wifi_rx_window,
        &report.wifi_rx_run,
        wifi_rx_counters,
    )?;
    let rx = &report.rx;
    if let Some(last) = rx.last_us
        && last > report.span.run_to_us
    {
        return Err(format!(
            "@RX: the last datagram at {last} us, after the run ended at {} us",
            report.span.run_to_us
        ));
    }
    // Every datagram the peer's counter took arrived through the IP stack's
    // UDP input within the run.
    let arrivals = [
        rx.datagrams,
        rx.duplicates,
        rx.untracked,
        rx.foreign,
        rx.echoed,
        rx.echo_refused,
    ]
    .iter()
    .try_fold(0_u64, |sum, count| sum.checked_add(*count))
    .ok_or("@RX: the arrivals overflow")?;
    if arrivals > report.lwip_run.udp_recv {
        return Err(format!(
            "@RX counts {arrivals} arrivals, the IP stack received {} UDP datagrams in the run",
            report.lwip_run.udp_recv
        ));
    }
    Ok(report)
}

impl Report {
    pub fn json(&self) -> Value {
        let lwip = |lwip: &Lwip| json!(lwip_counters(lwip).into_iter().collect::<BTreeMap<_, _>>());
        let wifi =
            |rx: &WifiRx| json!(wifi_rx_counters(rx).into_iter().collect::<BTreeMap<_, _>>());
        let rx = &self.rx;
        json!({
            "rx": {
                "datagrams": rx.datagrams,
                "bytes": rx.bytes,
                "window_datagrams": rx.window_datagrams,
                "window_bytes": rx.window_bytes,
                "lost": rx.lost,
                "reordered": rx.reordered,
                "duplicates": rx.duplicates,
                "untracked": rx.untracked,
                "highest": rx.highest,
                "foreign": rx.foreign,
                "echoed": rx.echoed,
                "echo_refused": rx.echo_refused,
                "receive_errors": rx.receive_errors,
                "first_us": rx.first_us,
                "last_us": rx.last_us,
                "window_us": rx.window_us,
            },
            "span": {
                "window_from_us": self.span.window_from_us,
                "window_to_us": self.span.window_to_us,
                "run_to_us": self.span.run_to_us,
            },
            "window": {
                "lwip": lwip(&self.lwip_window),
                "wifi_rx": wifi(&self.wifi_rx_window),
                "hw_rx": self.hw_rx,
            },
            "run": {
                "lwip": lwip(&self.lwip_run),
                "wifi_rx": wifi(&self.wifi_rx_run),
            },
            "link": {
                "channel": self.link.channel,
                "second": self.link.second,
                "rssi": self.link.rssi,
                "disconnects": self.link.disconnects,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peer_line::Line;

    const EXPECT: Expect = Expect {
        run: 7,
        delay_ms: 300,
        window_ms: 5000,
        drain_ms: 500,
        channel: 13,
        disconnects: 0,
    };

    const HW: &str = "@HWRX run=7 span=window rx_fcs_err=3 rx_abort=0 rx_abort_fcs_pass=0 \
        nrx_err_pwrdrop=0 rx_samebm_errcnt=0 rx_mpdu=50 rx_end_cnt=60 rx_datasuc=2 rx_cfo_hz=-4736 \
        rx_sf=1 rx_other_ucast=0 rx_buf_fullcnt=0 rx_fifo_ovfcnt=0 rx_tkip_errcnt=0 \
        rx_btblock_err=0 rx_freqhop_err=0 rx_lastunmatch_err=0 rx_ack_int_cnt=0 rx_rts_int_cnt=2 \
        brx_err_agc=0 brx_err=0 nrx_err=4 nrx_err_abort=0 nrx_err_agcexit=0 nrx_err_bboff=0 \
        nrx_err_fdm_wdg=0 nrx_err_restart=0 nrx_err_serv=0 nrx_err_txover=0 nrx_err_unsupport=0 \
        nrx_htsig_err=0 rxhung_statis=0 txhung_statis=0 rxtxhung=0 rxtxpanic=0";

    /// A whole, consistent report of run 7, as the peer prints it.
    fn report() -> Vec<String> {
        [
            "@RX run=7 datagrams=40 bytes=58880 window_datagrams=39 window_bytes=57408 lost=2 \
             reordered=1 duplicates=0 untracked=0 highest=41 foreign=0 echoed=5 echo_refused=0 \
             receive_errors=0 first_us=300100 last_us=5299000 window_us=5000000",
            "@SPAN run=7 window_from_us=306000 window_to_us=5306000 run_to_us=5806000",
            "@LWIP run=7 span=window link_recv=0 link_drop=0 ip_recv=44 ip_drop=0 udp_recv=44 \
             udp_drop=0 udp_memerr=0",
            "@LWIP run=7 span=run link_recv=0 link_drop=0 ip_recv=46 ip_drop=0 udp_recv=45 \
             udp_drop=0 udp_memerr=0",
            "@WIFIRX run=7 span=window tid=0 legacy=0 legacy_noeb=0 ht=44 ht_noeb=0 ht_stbc=0 \
             ht_retry=1 su=0 su_retry=0 rx_isr=0 rx_nblks=0 rx_reset_rxbase_cnt=0 \
             rx_base_null_cnt=0",
            "@WIFIRX run=7 span=run tid=0 legacy=0 legacy_noeb=0 ht=46 ht_noeb=0 ht_stbc=0 \
             ht_retry=1 su=0 su_retry=0 rx_isr=0 rx_nblks=0 rx_reset_rxbase_cnt=0 \
             rx_base_null_cnt=0",
            HW,
            "@LINK run=7 state=up channel=13 second=2 rssi=-30 disconnects=0",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn parsed(lines: &[String]) -> Vec<peer_line::Report> {
        lines
            .iter()
            .map(|line| match peer_line::parse(line) {
                Some(Line::Report(report)) => report,
                other => panic!("not a report line: {line} {other:?}"),
            })
            .collect()
    }

    fn decoded(lines: &[String]) -> Result<Report, String> {
        decode(&parsed(lines), &EXPECT)
    }

    /// The report with `from` replaced by `to` in its line `index`.
    fn with(index: usize, from: &str, to: &str) -> Vec<String> {
        let mut lines = report();
        assert!(lines[index].contains(from), "{from} in {}", lines[index]);
        lines[index] = lines[index].replace(from, to);
        lines
    }

    #[test]
    fn a_whole_consistent_report_is_decoded() {
        let report = decoded(&report()).unwrap();
        assert_eq!(report.rx.datagrams, 40);
        assert_eq!(report.rx.highest, Some(41));
        assert_eq!(report.wifi_rx_run.ht, 46);
        assert_eq!(report.lwip_window.udp_recv, 44);
        assert_eq!(report.hw_rx["rx_cfo_hz"], -4736);
        assert_eq!(report.link.channel, 13);
    }

    #[test]
    fn a_window_other_than_the_requested_one_is_refused() {
        for (index, from, to) in [
            (0, "window_us=5000000", "window_us=0"),
            (0, "window_us=5000000", "window_us=4999999"),
            (1, "window_from_us=306000", "window_from_us=299000"),
            (1, "window_from_us=306000", "window_from_us=400000"),
            (1, "window_to_us=5306000", "window_to_us=5000000"),
            (1, "run_to_us=5806000", "run_to_us=9000000"),
            (1, "run_to_us=5806000", "run_to_us=5700000"),
        ] {
            assert!(decoded(&with(index, from, to)).is_err(), "{to}");
        }
    }

    #[test]
    fn a_count_is_unsigned_and_a_time_is_minus_one_or_unsigned() {
        for (index, from, to) in [
            (3, "udp_drop=0", "udp_drop=-1"),
            (0, "datagrams=40", "datagrams=-40"),
            (0, "first_us=300100", "first_us=-2"),
            (0, "highest=41", "highest=-5"),
            (4, "ht=44", "ht=4.4"),
            (6, "rx_mpdu=50", "rx_mpdu=70000"),
            (6, "rx_cfo_hz=-4736", "rx_cfo_hz=40000"),
            (7, "rssi=-30", "rssi=12"),
        ] {
            assert!(decoded(&with(index, from, to)).is_err(), "{to}");
        }
    }

    #[test]
    fn a_line_of_another_run_a_second_line_or_an_unknown_line_is_refused() {
        let mut stale = report();
        stale.push("@RX run=6 datagrams=1 bytes=1472 window_datagrams=1 window_bytes=1472 lost=0 reordered=0 duplicates=0 untracked=0 highest=0 foreign=0 echoed=0 echo_refused=0 receive_errors=0 first_us=1 last_us=1 window_us=5000000".to_owned());
        let error = decoded(&stale).unwrap_err();
        assert!(error.contains("of run `6`"), "{error}");
        let mut twice = report();
        twice.push(twice[1].clone());
        assert!(decoded(&twice).unwrap_err().contains("twice"));
        let mut lost = report();
        lost.insert(3, "@LOST state=down reason=8 disconnects=1".to_owned());
        assert!(decoded(&lost).unwrap_err().contains("unexpected @LOST"));
        let mut other_span = report();
        other_span.push("@HWRX run=7 span=run rx_fcs_err=0".to_owned());
        assert!(
            decoded(&other_span)
                .unwrap_err()
                .contains("unexpected @HWRX span=run")
        );
        let mut missing = report();
        missing.remove(5);
        assert!(
            decoded(&missing)
                .unwrap_err()
                .contains("no @WIFIRX span=run")
        );
    }

    #[test]
    fn a_line_holds_exactly_its_fields_each_once() {
        for (index, from, to) in [
            (0, "lost=2 ", ""),
            (0, "lost=2 ", "lost=2 lost=2 "),
            (2, "udp_memerr=0", "udp_memerr=0 extra=1"),
            (7, "disconnects=0", "disconnects=0 down"),
            (4, "tid=0", "tid=7"),
        ] {
            assert!(decoded(&with(index, from, to)).is_err(), "{to}");
        }
    }

    #[test]
    fn the_counts_must_agree_with_each_other() {
        for (index, from, to) in [
            // The tally's own arithmetic.
            (0, "lost=2", "lost=3"),
            (0, "window_datagrams=39", "window_datagrams=41"),
            (0, "bytes=58880", "bytes=100"),
            (0, "reordered=1", "reordered=40"),
            (0, "first_us=300100", "first_us=5300000"),
            (0, "highest=41", "highest=-1"),
            (0, "last_us=5299000", "last_us=6000000"),
            // A window within its run, a part within its whole.
            (2, "ip_recv=44", "ip_recv=47"),
            (4, "ht=44", "ht=47"),
            (5, "ht_noeb=0", "ht_noeb=47"),
            (3, "udp_recv=45", "udp_recv=44"),
            (3, "ip_recv=46", "ip_recv=44"),
        ] {
            assert!(decoded(&with(index, from, to)).is_err(), "{to}");
        }
    }

    #[test]
    fn the_link_must_be_up_on_the_joined_channel_without_a_new_disconnection() {
        for (from, to) in [
            ("state=up channel=13 second=2 rssi=-30", "state=down"),
            ("channel=13", "channel=6"),
            ("second=2", "second=3"),
            ("disconnects=0", "disconnects=1"),
        ] {
            assert!(decoded(&with(7, from, to)).is_err(), "{to}");
        }
    }

    #[test]
    fn a_run_without_datagrams_is_consistent_only_without_counts() {
        let empty = with(
            0,
            "datagrams=40 bytes=58880 window_datagrams=39 window_bytes=57408 lost=2 reordered=1 duplicates=0 untracked=0 highest=41",
            "datagrams=0 bytes=0 window_datagrams=0 window_bytes=0 lost=0 reordered=0 duplicates=0 untracked=0 highest=-1",
        );
        let mut empty = empty;
        empty[0] = empty[0].replace("first_us=300100 last_us=5299000", "first_us=-1 last_us=-1");
        assert_eq!(decoded(&empty).unwrap().rx.datagrams, 0);
        let mut lost = empty.clone();
        lost[0] = lost[0].replace("lost=0", "lost=3");
        assert!(decoded(&lost).is_err());
    }

    #[test]
    fn the_join_names_its_channel_and_address_once() {
        let line = |text: &str| parsed(&[text.to_owned()]);
        let good = "@JOINED channel=13 second=2 rssi=-25 phymode=5 ip=10.231.0.144 disconnects=0";
        let joined = joined(&line(good), 13).unwrap();
        assert_eq!(joined.address, std::net::Ipv4Addr::new(10, 231, 0, 144));
        assert!(super::joined(&line(good), 6).is_err());
        for bad in [
            "@JOINED channel=13 second=2 rssi=-25 phymode=5 ip=0.0.0.0 disconnects=0",
            "@JOINED channel=13 second=2 rssi=-25 phymode=5 ip=10.231.0 disconnects=0",
            "@JOINED channel=13 second=2 rssi=-25 phymode=5 disconnects=0",
            "@JOINED channel=13 second=2 rssi=-25 phymode=5 ip=10.231.0.144 disconnects=-1",
        ] {
            assert!(super::joined(&line(bad), 13).is_err(), "{bad}");
        }
        let twice = parsed(&[good.to_owned(), good.to_owned()]);
        assert!(super::joined(&twice, 13).is_err());
    }
}
