//! Checker policy for named diagnostic calls; Blobray supplies the bit flow.
use oer_analysis_blobray_domain::{CallTarget, ReadSink, ReadUses};
use serde::Serialize;
use std::collections::BTreeMap;

/// Exact names only, with chip-local pinned symbols/calls inspected by Blobray.
/// Name-only external callees are intentional; unnamed calls remain uses.
type Diagnostics = &'static [(&'static str, &'static str)];

const S31_DIAGNOSTICS: Diagnostics = &[
    // SOURCE: pin:esp32s31/rom::ets_printf, ets_vprintf.
    ("ets_printf", "pin:esp32s31/rom::ets_printf"),
    ("ets_vprintf", "pin:esp32s31/rom::ets_vprintf"),
    // SOURCE: call at phy_tx_gain_print + 144 in pin:esp32s31/libphy.
    ("phy_printf", "pin:esp32s31/libphy::phy_tx_gain_print+144"),
    // SOURCE: call at dbg_dump_rx_links + 138 in pin:esp32s31/libpp.
    ("pp_printf", "pin:esp32s31/libpp::dbg_dump_rx_links+138"),
    // SOURCE: pin:esp32s31/libphy[phy_debug.o]::phy_log.
    ("phy_log", "pin:esp32s31/libphy[phy_debug.o]::phy_log"),
    // SOURCE: pin:esp32s31/libnet80211[ieee80211_debug.o]::wifi_log.
    (
        "wifi_log",
        "pin:esp32s31/libnet80211[ieee80211_debug.o]::wifi_log",
    ),
    // SOURCE: named log entry points in pin:esp32s31/libble_app[ble_71.o].
    (
        "r_ble_log_internal_x0",
        "pin:esp32s31/libble_app[ble_71.o]::r_ble_log_internal_x0",
    ),
    (
        "r_ble_log_internal_x1",
        "pin:esp32s31/libble_app[ble_71.o]::r_ble_log_internal_x1",
    ),
    (
        "r_ble_log_internal_x2",
        "pin:esp32s31/libble_app[ble_71.o]::r_ble_log_internal_x2",
    ),
    (
        "r_ble_log_internal_x3",
        "pin:esp32s31/libble_app[ble_71.o]::r_ble_log_internal_x3",
    ),
    (
        "r_ble_log_internal_hex",
        "pin:esp32s31/libble_app[ble_71.o]::r_ble_log_internal_hex",
    ),
    // SOURCE: named log entry points in pin:esp32s31/libbtdm_common[13.o].
    (
        "wr_btdm_log_internal_x0",
        "pin:esp32s31/libbtdm_common[13.o]::wr_btdm_log_internal_x0",
    ),
    (
        "wr_btdm_log_internal_x1",
        "pin:esp32s31/libbtdm_common[13.o]::wr_btdm_log_internal_x1",
    ),
    (
        "wr_btdm_log_internal_x2",
        "pin:esp32s31/libbtdm_common[13.o]::wr_btdm_log_internal_x2",
    ),
    (
        "wr_btdm_log_internal_x3",
        "pin:esp32s31/libbtdm_common[13.o]::wr_btdm_log_internal_x3",
    ),
    (
        "wr_btdm_log_internal_hex",
        "pin:esp32s31/libbtdm_common[13.o]::wr_btdm_log_internal_hex",
    ),
    // SOURCE: diagnostic assert wrappers in pin:esp32s31/libbtdm_common[0.o].
    (
        "wr_btdm_compressed_assert_x0",
        "pin:esp32s31/libbtdm_common[0.o]::wr_btdm_compressed_assert_x0",
    ),
    (
        "wr_btdm_compressed_assert_x1",
        "pin:esp32s31/libbtdm_common[0.o]::wr_btdm_compressed_assert_x1",
    ),
    (
        "wr_btdm_compressed_assert_x2",
        "pin:esp32s31/libbtdm_common[0.o]::wr_btdm_compressed_assert_x2",
    ),
];

/// Selection happens once at the checker boundary; unsupported chips fail.
pub struct Policy {
    diagnostics: Diagnostics,
}
impl Policy {
    pub fn for_chip(chip: &str) -> crate::Result<Self> {
        let diagnostics = match chip {
            "esp32s31" => S31_DIAGNOSTICS,
            _ => return Err("no diagnostic policy for this chip".into()),
        };
        Ok(Self { diagnostics })
    }
    pub fn source(&self, target: Option<&CallTarget>) -> Option<&'static str> {
        let name = target?.name.as_deref()?;
        self.diagnostics
            .iter()
            .find_map(|(n, source)| (*n == name).then_some(*source))
    }
    pub fn diagnostic(&self, sink: &ReadSink) -> bool {
        matches!(sink, ReadSink::CallArgument { callee, .. } if self.source(callee.as_ref()).is_some())
    }
}

#[derive(Debug, Serialize)]
pub struct Callee {
    pub target: Option<CallTarget>,
    pub diagnostic_source: Option<&'static str>,
    /// Union per load and callee, then sum over load observations.
    pub bits: u64,
    /// Bits reaching only this callee, with no other call or non-call sink.
    pub exclusive_bits: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Accounting {
    pub loads: u64,
    pub load_bits: u64,
    pub producer_used_bits: u64,
    pub behavior_used_bits: u64,
    pub call_bits: u64,
    pub diagnostic_call_bits: u64,
    pub diagnostic_only_bits: u64,
    pub callees: Vec<Callee>,
    #[serde(skip)]
    grouped: BTreeMap<Option<CallTarget>, (u64, u64)>,
}

impl Accounting {
    pub fn observe(&mut self, reads: &ReadUses, full: u32, policy: &Policy) {
        self.loads += 1;
        self.load_bits += u64::from(full.count_ones());
        let (mut used, mut behavior, mut diagnostics, mut noncall) = (0, 0, 0, 0);
        let mut calls = BTreeMap::<Option<CallTarget>, u32>::new();
        for item in &reads.uses {
            used |= item.bits;
            if policy.diagnostic(&item.sink) {
                diagnostics |= item.bits;
            } else {
                behavior |= item.bits;
            }
            match &item.sink {
                ReadSink::CallArgument { callee, .. } => {
                    *calls.entry(callee.clone()).or_default() |= item.bits;
                }
                _ => noncall |= item.bits,
            }
        }
        let call = calls.values().fold(0, |bits, next| bits | next);
        self.producer_used_bits += u64::from(used.count_ones());
        self.behavior_used_bits += u64::from(behavior.count_ones());
        self.call_bits += u64::from(call.count_ones());
        self.diagnostic_call_bits += u64::from(diagnostics.count_ones());
        self.diagnostic_only_bits += u64::from((diagnostics & !behavior).count_ones());
        for (target, bits) in &calls {
            let other = calls
                .iter()
                .filter(|(key, _)| *key != target)
                .fold(noncall, |mask, (_, bits)| mask | bits);
            let count = self.grouped.entry(target.clone()).or_default();
            count.0 += u64::from(bits.count_ones());
            count.1 += u64::from((bits & !other).count_ones());
        }
    }

    pub fn finish(&mut self, policy: &Policy) {
        self.callees = std::mem::take(&mut self.grouped)
            .into_iter()
            .map(|(target, (bits, exclusive_bits))| Callee {
                diagnostic_source: policy.source(target.as_ref()),
                target,
                bits,
                exclusive_bits,
            })
            .collect();
        self.callees
            .sort_by(|a, b| b.bits.cmp(&a.bits).then_with(|| a.target.cmp(&b.target)));
    }

    pub fn render(&self, text: &mut String, scope: &str) {
        text.push_str(&format!("\nREAD ACCOUNTING {scope}: {} loads / {} bits; producer-used {} behavior-used {}\n  diagnostic call bits {} / {} call bits; diagnostic-only {} / {} load bits\n  Counts union each load's source bits before indexed address expansion; callee groups may overlap.\n", self.loads, self.load_bits, self.producer_used_bits, self.behavior_used_bits, self.diagnostic_call_bits, self.call_bits, self.diagnostic_only_bits, self.load_bits));
        for entry in self.callees.iter().take(20) {
            text.push_str(&format!(
                "  CALLEE {:?}: {} bits, {} exclusive; diagnostic SOURCE {:?}\n",
                entry.target, entry.bits, entry.exclusive_bits, entry.diagnostic_source
            ));
        }
    }
}
