use core::fmt::{self, Write};

#[derive(Clone, Copy, Default)]
pub struct Visit {
    pub channel: u8,
    pub allowed: bool,
    pub sent: u8,
    pub success: bool,
    pub responses: u32,
    pub rate_kbps: u32,
    pub retune_us: u64,
    pub power: [i8; 4],
    pub ceiling: i8,
}
pub struct Report {
    pub visits: [Visit; 13],
    pub count: usize,
    pub lab_channel: u8,
    pub country: [u8; 2],
    pub cap_dbm: i8,
    pub target: [u8; 2],
    pub stand: [u8; 2],
    pub unicast_rate: u32,
    pub unicast_sequence: u16,
    pub unicast_success: bool,
    pub unicast_responses: u32,
    pub unicast_attempts: u8,
    pub unicast_raw: u8,
    pub unicast_power: [i8; 4],
    pub unicast_ceiling: i8,
    pub timeouts: u32,
    pub collisions: u32,
    pub colors: u32,
    pub errors: u32,
    pub error: Option<&'static str>,
}
impl Report {
    pub fn new(config: crate::W2Config, station: [u8; 6]) -> Self {
        Self {
            visits: [Visit::default(); 13],
            count: 0,
            lab_channel: config.channel,
            country: config.country,
            cap_dbm: config.cap_dbm,
            target: [station[4], station[5]],
            stand: [config.stand[4], config.stand[5]],
            unicast_rate: 0,
            unicast_sequence: 0,
            unicast_success: false,
            unicast_responses: 0,
            unicast_attempts: 0,
            unicast_raw: 255,
            unicast_power: [0; 4],
            unicast_ceiling: 0,
            timeouts: 0,
            collisions: 0,
            colors: 0,
            errors: 0,
            error: None,
        }
    }
    pub fn gates(&self) -> [bool; 5] {
        [
            self.visits[..self.count]
                .iter()
                .any(|v| v.channel == self.lab_channel && v.responses > 0),
            self.count > 0
                && self.visits[..self.count].iter().all(|v| {
                    if v.allowed {
                        v.sent == 1 && v.success
                    } else {
                        v.sent == 0
                    }
                }),
            self.unicast_success
                && self.unicast_responses > 0
                && (1..=7).contains(&self.unicast_attempts),
            self.error.is_none()
                && self.timeouts == 0
                && self.collisions == 0
                && self.colors == 0
                && self.errors == 0,
            self.unicast_success
                && self
                    .unicast_power
                    .iter()
                    .all(|p| *p <= self.unicast_ceiling && *p <= self.cap_dbm)
                && self.visits[..self.count]
                    .iter()
                    .filter(|v| v.sent > 0)
                    .all(|v| {
                        v.power
                            .iter()
                            .all(|p| *p <= v.ceiling && *p <= self.cap_dbm)
                    }),
        ]
    }
    pub fn print(&self, out: &mut impl Write) -> fmt::Result {
        let gates = self.gates();
        writeln!(out, "OER-W2-BEGIN v=1\r")?;
        writeln!(
            out,
            "w2.config channel={} country={}{} cap={} target=xx:xx:xx:xx:{:02X}:{:02X} stand=xx:xx:xx:xx:{:02X}:{:02X}\r",
            self.lab_channel,
            self.country[0] as char,
            self.country[1] as char,
            self.cap_dbm,
            self.target[0],
            self.target[1],
            self.stand[0],
            self.stand[1]
        )?;
        for v in &self.visits[..self.count] {
            writeln!(
                out,
                "w2.channel channel={} allowed={} sent={} success={} responses={} rate_kbps={} retune_us={} power={},{},{},{} ceiling={}\r",
                v.channel,
                u8::from(v.allowed),
                v.sent,
                u8::from(v.success),
                v.responses,
                v.rate_kbps,
                v.retune_us,
                v.power[0],
                v.power[1],
                v.power[2],
                v.power[3],
                v.ceiling
            )?;
        }
        writeln!(
            out,
            "w2.unicast rate_kbps={} sequence={} success={} responses={} attempts={} raw={} power={},{},{},{} ceiling={}\r",
            self.unicast_rate,
            self.unicast_sequence,
            u8::from(self.unicast_success),
            self.unicast_responses,
            self.unicast_attempts,
            self.unicast_raw,
            self.unicast_power[0],
            self.unicast_power[1],
            self.unicast_power[2],
            self.unicast_power[3],
            self.unicast_ceiling
        )?;
        writeln!(
            out,
            "w2.stats timeouts={} collisions={} colors={} errors={} error={}\r",
            self.timeouts,
            self.collisions,
            self.colors,
            self.errors,
            self.error.unwrap_or("-")
        )?;
        for (name, pass) in ['a', 'b', 'c', 'd', 'e'].into_iter().zip(gates) {
            writeln!(out, "w2.gate name={name} pass={}\r", u8::from(pass))?;
        }
        writeln!(out, "OER-W2-END lines={}\r", self.count + 8)?;
        writeln!(
            out,
            "OER-RESULT: {} w2\r",
            if gates.into_iter().all(|g| g) && self.error.is_none() {
                "pass"
            } else {
                "fail"
            }
        )
    }
}
