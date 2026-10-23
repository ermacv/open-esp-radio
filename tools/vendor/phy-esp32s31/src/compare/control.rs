//! Bounds supplied only by the complete alternating series being compared.
use super::{Images, Readings, Register, VendorBoot, Verdict};
use oer_vendor_phy::calibration_capture::boots::Lifecycle;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterReference {
    SameRunControl,
}

/// Primary vendor captures from the same complete alternating series.
/// Archived callers obtain this only from the authenticated run reader.
#[derive(Debug)]
pub struct SameRunControl {
    pub(crate) boots: Vec<VendorBoot>,
    identity: Images,
}

impl SameRunControl {
    pub(crate) fn alternating(
        lifecycle: Lifecycle,
        identity: &Images,
        vendor: Vec<VendorBoot>,
        production_count: usize,
    ) -> Option<Self> {
        let required = if lifecycle.ieee802154() { 5 } else { 10 };
        (vendor.len() >= required && vendor.len() == production_count).then_some(Self {
            boots: vendor,
            identity: identity.clone(),
        })
    }

    pub(crate) fn belongs_to(&self, identity: &Images, production_count: usize) -> bool {
        &self.identity == identity && self.boots.len() == production_count
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ControlRegister {
    pub name: String,
    pub reason: String,
    pub vendor_captures: usize,
    pub vendor: Option<[u32; 2]>,
    pub production: Option<[u32; 2]>,
    pub margin: Option<u64>,
    pub verdict: Verdict,
}

pub(super) fn range(readings: &[&Readings], register: &Register) -> Option<[u32; 2]> {
    let mut bounds = [u32::MAX, u32::MIN];
    if readings.is_empty() {
        return None;
    }
    for reading in readings {
        if reading.unreadable.contains(&register.address) {
            return None;
        }
        let value = reading.values.get(&register.address)? & register.mask;
        bounds = [bounds[0].min(value), bounds[1].max(value)];
    }
    Some(bounds)
}

pub(super) fn compare(
    register: &Register,
    reason: &str,
    vendor: Option<&[&Readings]>,
    production: &[&Readings],
) -> ControlRegister {
    let v = vendor.and_then(|readings| range(readings, register));
    let p = range(production, register);
    let margin = v.map(|[a, b]| u64::from(b - a));
    let verdict = match (v, p, margin) {
        (Some(v), Some(p), Some(margin))
            if u64::from(v[0]) <= u64::from(p[0]) + margin
                && u64::from(p[1]) <= u64::from(v[1]) + margin =>
        {
            Verdict::Match
        }
        (Some(_), Some(_), Some(_)) => Verdict::Diff,
        _ => Verdict::Incomplete,
    };
    ControlRegister {
        name: register.name.clone(),
        reason: reason.into(),
        vendor_captures: vendor.map_or(0, <[_]>::len),
        vendor: v,
        production: p,
        margin,
        verdict,
    }
}

#[cfg(test)]
mod tests;
