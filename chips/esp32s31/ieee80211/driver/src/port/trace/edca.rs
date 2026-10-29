//! Read-only records from the policy that selected the actual publication.
use super::*;
use crate::mac::tx::{LegacyTxQueue, runtime::WifiTxRuntimePolicy};
use oer_ieee80211_port::{EdcaAcParameters, VifRole};
use oer_ieee80211_wire::{extensions::wmm::WmmAcParameters, qos::WmmAccessCategory};
fn role(value: u32) -> VifRole {
    if value == 0 {
        VifRole::Station
    } else {
        VifRole::AccessPoint
    }
}
fn category(value: u32) -> WmmAccessCategory {
    match value {
        0 => WmmAccessCategory::BestEffort,
        1 => WmmAccessCategory::Background,
        2 => WmmAccessCategory::Video,
        _ => WmmAccessCategory::Voice,
    }
}
pub struct EdcaTable {
    pub role: VifRole,
    pub count: u8,
    pub uapsd: bool,
}
impl Event for EdcaTable {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 103);
    const CHANNEL: Channel = Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [
            self.role as u32,
            u32::from(self.count) | (u32::from(self.uapsd) << 8),
        ]
    }
    fn decode([r, v]: [u32; 2]) -> Option<Self> {
        if r > 1 || v > 0x1ff {
            return None;
        }
        Some(Self {
            role: role(r),
            count: v as u8,
            uapsd: v & 0x100 != 0,
        })
    }
}
pub struct EdcaRecord {
    pub role: VifRole,
    pub category: WmmAccessCategory,
    pub parameters: EdcaAcParameters,
}
impl Event for EdcaRecord {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 104);
    const CHANNEL: Channel = Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        let w = self.parameters.wmm;
        [
            u32::from(w.aifsn.get())
                | (u32::from(w.ecw_min.get()) << 4)
                | (u32::from(w.ecw_max.get()) << 8)
                | (u32::from(w.txop_limit_units_32_us) << 12)
                | (u32::from(w.admission_control_mandatory) << 28)
                | ((self.role as u32) << 29)
                | ((self.category as u32) << 30),
            u32::try_from(self.parameters.msdu_lifetime.as_micros())
                .expect("installed S31 MSDU lifetime fits u32"),
        ]
    }
    fn decode([w, life]: [u32; 2]) -> Option<Self> {
        Some(Self {
            role: role((w >> 29) & 1),
            category: category(w >> 30),
            parameters: EdcaAcParameters {
                wmm: WmmAcParameters {
                    admission_control_mandatory: w & (1 << 28) != 0,
                    aifsn: oer_wire::U8::from_low_bits((w & 15) as u8),
                    ecw_min: oer_wire::U8::from_low_bits(((w >> 4) & 15) as u8),
                    ecw_max: oer_wire::U8::from_low_bits(((w >> 8) & 15) as u8),
                    txop_limit_units_32_us: ((w >> 12) & 0xffff) as u16,
                },
                msdu_lifetime: oer_time::Duration::from_micros(u64::from(life)),
            },
        })
    }
}
pub struct EdcaDraw {
    pub role: VifRole,
    pub category: WmmAccessCategory,
    pub exponent: u8,
    pub backoff: u16,
}
impl Event for EdcaDraw {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 105);
    const CHANNEL: Channel = Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [
            self.role as u32 | ((self.category as u32) << 1) | (u32::from(self.exponent) << 3),
            u32::from(self.backoff),
        ]
    }
    fn decode([v, b]: [u32; 2]) -> Option<Self> {
        if v > 127 {
            return None;
        }
        Some(Self {
            role: role(v & 1),
            category: category((v >> 1) & 3),
            exponent: (v >> 3) as u8,
            backoff: b.try_into().ok()?,
        })
    }
}
#[inline(never)]
pub(crate) fn contention(
    policy: &WifiTxRuntimePolicy,
    head: VifRole,
    ac: WmmAccessCategory,
    backoff: u16,
) {
    if !oer_espressif_staged_runtime::trace::enabled(Publication::CHANNEL) {
        return;
    }
    for role in [VifRole::Station, VifRole::AccessPoint] {
        let set = policy.debug_edca(role);
        let wmm = set.wmm();
        oer_espressif_staged_runtime::trace::emit(&EdcaTable {
            role,
            count: wmm.parameter_set_count,
            uapsd: wmm.uapsd,
        });
        for category in WmmAccessCategory::ALL {
            oer_espressif_staged_runtime::trace::emit(&EdcaRecord {
                role,
                category,
                parameters: set.access_category(category),
            });
        }
    }
    oer_espressif_staged_runtime::trace::emit(&EdcaDraw {
        role: head,
        category: ac,
        exponent: policy.contention_exponent(LegacyTxQueue::from_access_category(ac)),
        backoff,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_keep_every_vif_ac_field_and_the_actual_window_and_draw() {
        for role in [VifRole::Station, VifRole::AccessPoint] {
            for category in WmmAccessCategory::ALL {
                for updated in [false, true] {
                    let mut parameters = oer_ieee80211_port::EdcaParameterSet::STANDARD_DEFAULT
                        .access_category(category);
                    if updated {
                        parameters.wmm.admission_control_mandatory = true;
                        parameters.wmm.aifsn = oer_wire::U8::constant::<15>();
                        parameters.wmm.ecw_min = oer_wire::U8::constant::<6>();
                        parameters.wmm.ecw_max = oer_wire::U8::constant::<10>();
                        parameters.wmm.txop_limit_units_32_us = 350;
                    }
                    let input = EdcaRecord {
                        role,
                        category,
                        parameters,
                    };
                    let output = EdcaRecord::decode(input.encode()).expect("record roundtrip");
                    assert_eq!(output.role, role);
                    assert_eq!(output.category, category);
                    assert_eq!(output.parameters, parameters);
                    let draw = EdcaDraw {
                        role,
                        category,
                        exponent: 10,
                        backoff: 1023,
                    };
                    let output = EdcaDraw::decode(draw.encode()).expect("draw roundtrip");
                    assert_eq!(
                        (
                            output.role,
                            output.category,
                            output.exponent,
                            output.backoff
                        ),
                        (role, category, 10, 1023)
                    );
                }
            }
            let table = EdcaTable {
                role,
                count: 15,
                uapsd: true,
            };
            let output = EdcaTable::decode(table.encode()).expect("table roundtrip");
            assert_eq!((output.role, output.count, output.uapsd), (role, 15, true));
        }
    }
}
