use oer_diagnostics::Note;

#[derive(Clone, Copy, Default)]
pub struct Visit {
    pub channel: u8,
    pub sequence: u16,
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
    pub antenna_mbm: i32,
    pub chip_mbm: i32,
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
    pub irq_free: usize,
    pub supported: [u8; 8],
    pub supported_len: usize,
    pub extended: [u8; 16],
    pub extended_len: usize,
    pub error: Option<oer_diagnostics::Name<32>>,
}
impl Report {
    pub fn new(config: crate::W2Config, station: [u8; 6]) -> Self {
        Self {
            visits: [Visit::default(); 13],
            count: 0,
            lab_channel: config.channel,
            country: config.country,
            cap_dbm: config.cap_dbm,
            antenna_mbm: oer_esp32s31_board::WIFI_ANTENNA_GAIN_MBM,
            chip_mbm: i32::from(
                oer_esp32s31_phy::PhyConfig::production().highest_target_power_quarter_dbm(),
            ) * 25,
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
            irq_free: 0,
            supported: [0; 8],
            supported_len: 0,
            extended: [0; 16],
            extended_len: 0,
            error: None,
        }
    }
    pub fn gates(&self) -> [bool; 5] {
        [
            self.visits[..self.count]
                .iter()
                .any(|v| v.channel == self.lab_channel && v.responses > 0),
            self.error.is_none()
                && self.count > 0
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
    /// The report block as typed notes: the host renders the console text
    /// `cargo fw w2 check` reads. A channel visit is two notes of one line.
    pub async fn notes<E>(
        &self,
        mut emit: impl AsyncFnMut(&Note<'static>) -> Result<(), E>,
    ) -> Result<(), E> {
        let gates = self.gates();
        emit(&Note::w2_begin()).await?;
        emit(&Note::w2_config(
            u32::from(self.lab_channel),
            self.country,
            i32::from(self.cap_dbm),
            self.antenna_mbm,
            self.chip_mbm,
            self.target,
            self.stand,
        ))
        .await?;
        for v in self.visits.iter().take(self.count) {
            emit(&Note::w2_channel_first(
                u32::from(v.channel),
                u32::from(v.sequence),
                v.allowed,
                u32::from(v.sent),
                v.success,
                v.responses,
            ))
            .await?;
            emit(&Note::w2_channel_last(
                v.rate_kbps,
                v.retune_us,
                v.power,
                i32::from(v.ceiling),
            ))
            .await?;
        }
        emit(&Note::w2_basic(
            self.supported.get(..self.supported_len).unwrap_or(&[]),
            self.extended.get(..self.extended_len).unwrap_or(&[]),
        ))
        .await?;
        emit(&Note::w2_unicast(
            self.unicast_rate,
            u32::from(self.unicast_sequence),
            self.unicast_success,
            self.unicast_responses,
            u32::from(self.unicast_attempts),
            u32::from(self.unicast_raw),
            self.unicast_power,
            i32::from(self.unicast_ceiling),
        ))
        .await?;
        emit(&Note::w2_stats(
            self.timeouts,
            self.collisions,
            self.colors,
            self.errors,
            self.irq_free as u32,
            self.error.unwrap_or(oer_diagnostics::name!("-"; 32)),
        ))
        .await?;
        for (name, pass) in [
            oer_diagnostics::name!("a"; 32),
            oer_diagnostics::name!("b"; 32),
            oer_diagnostics::name!("c"; 32),
            oer_diagnostics::name!("d"; 32),
            oer_diagnostics::name!("e"; 32),
        ]
        .into_iter()
        .zip(gates)
        {
            emit(&Note::w2_gate(name, pass)).await?;
        }
        emit(&Note::w2_end(self.count as u32 + 9)).await?;
        emit(&if gates.into_iter().all(|g| g) && self.error.is_none() {
            Note::w2_pass()
        } else {
            Note::w2_fail()
        })
        .await
    }
}
