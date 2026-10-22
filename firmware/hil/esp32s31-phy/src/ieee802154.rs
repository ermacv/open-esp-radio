//! Image-local RX diagnostic assembly; no block runtime API.
use crate::MacToken;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::{CoexAbsent, Coexisting, CommonPower, Ieee802154Client, RadioDomain},
    ieee802154::{
        Ieee802154Cold,
        engine::{self, DetachedIeee802154Engine, Ieee802154Engine},
        ll::Ieee802154MacOwners,
    },
    root::Ieee802154RadioPartition,
};
use oer_esp32s31_knowledge::tx_power::ESP32S31_TX_POWER_LEVELS;
use oer_esp32s31_phy::{
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyClientAccess, phy_ready},
    ieee802154_client::{
        Ieee802154PhyMembership, RegisteredIeee802154Operational,
        RegisteredIeee802154OperationalRoute, join_ieee802154, leave_ieee802154,
    },
    *,
};
use oer_espressif_esp_hal_radio::EspHalClocks;
use oer_espressif_ieee802154_engine::{
    channel::Ieee802154Channel,
    coex::{Ieee802154CoexPriorities, Ieee802154Coexistence},
    engine::{
        FRAME_SIZE, Ieee802154EngineBuffers, Ieee802154EnhancedAck, Ieee802154Environment,
        Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154State,
        Ieee802154TxError, RX_BUFFER_COUNT,
    },
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
    types::Ieee802154RxStatus,
};
use oer_hil_protocol::base::RejectReason;
use oer_memory::HandlerCell;
use oer_radio_regulatory::{
    Country, FrequencySpan, PowerLimits, RegulatoryDomain, Rule, RuleFlags,
};
use oer_time::Clock;
use static_cell::StaticCell;

type Result<T> = core::result::Result<T, RejectReason>;

// MAC IRQ and every engine entry run on the same hart at P2. They cannot
// preempt one another. No borrow crosses await; P1 and P3 never enter this
// cell. HandlerCell rejects reentry without waiting or sharing a lock.
static ENGINE: HandlerCell<Option<Running>> = HandlerCell::new();
static STORAGE: StaticCell<Option<Running>> = StaticCell::new();

struct Running {
    engine: Ieee802154Engine,
    env: Environment,
}
impl Running {
    fn recycle(&mut self) -> Result<()> {
        while let Some(slot) = self.env.received.pop() {
            self.engine
                .receive_handle_done(slot)
                .map_err(|_| RejectReason::Internal)?;
        }
        if self.env.failed {
            return Err(RejectReason::Internal);
        }
        Ok(())
    }
}

pub fn interrupt() {
    ENGINE
        .with(|running| {
            let running = running.as_mut().expect("routed MAC IRQ has an engine");
            running.engine.isr(&mut running.env);
            running
                .recycle()
                .expect("RX diagnostic returns every delivered slot");
        })
        .expect("P2 MAC entries cannot reenter");
}

pub struct Rx {
    cold: Option<Ieee802154Cold>,
    absent: Option<CoexAbsent<Ieee802154Client>>,
    coex: Option<Coexisting<Ieee802154Client>>,
    power: Option<CommonPower<Ieee802154Client>>,
    route: Option<RegisteredIeee802154OperationalRoute>,
    irq: MacToken,
    buffers: Option<oer_esp32s31_hal::dma_static::Placed<Ieee802154EngineBuffers>>,
    detached: Option<DetachedIeee802154Engine>,
}
impl Rx {
    pub fn new(
        partition: Ieee802154RadioPartition,
        absent: CoexAbsent<Ieee802154Client>,
        irq: MacToken,
    ) -> Self {
        assert!(
            ENGINE.install(STORAGE.init(None)).is_ok(),
            "one diagnostic MAC owner"
        );
        Self {
            cold: Some(Ieee802154Cold::from_partition(partition)),
            absent: Some(absent),
            coex: None,
            power: None,
            route: None,
            irq,
            buffers: Some(engine::claim_buffers().expect("one placed MAC storage claim")),
            detached: None,
        }
    }

    pub async fn enter(
        &mut self,
        domain: &mut RadioDomain<oer_espressif_esp_hal_radio::EspHal, ConcurrentPhy>,
        clocks: &EspHalClocks,
        channel: u8,
        power_dbm: i8,
    ) -> Result<()> {
        let permit = receive_permit(channel)?;
        let absent = self.absent.take().ok_or(RejectReason::Internal)?;
        let (coex, _) = domain.enter_coex(absent);
        self.coex = Some(coex);
        let (powered, power) = self
            .cold
            .take()
            .ok_or(RejectReason::Internal)?
            .power_up(domain, clocks)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-power-up"))?;
        self.power = Some(power);
        crate::tx_rate_checkpoint::observe(
            domain,
            crate::tx_rate_checkpoint::Point::AfterIeee802154CommonPower,
        )?;
        let clocked = powered
            .enable_clocks(domain, clocks)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-clocks-on"))?;
        let ready =
            phy_ready(domain).map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-phy-ready"))?;
        let PhyClientAccess::Available(available) = ready.client::<Ieee802154PhyMembership>()
        else {
            return Err(RejectReason::Internal);
        };
        let joined = join_ieee802154(available, &clocked, &EmbassyClock).map_err(|error| {
            oer_esp32s31_board::rom::print_fmt(format_args!(
                "OPEN_RADIO_HIL phy=FAIL step=154-join error={error:?}\n"
            ));
            RejectReason::Internal
        })?;
        oer_esp32s31_board::rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL phy=joined acquired={:?}\n",
            joined.acquired
        ));
        let due = joined.acquired == ConcurrentAcquire::TrackingDue;
        let member = joined.membership;
        if due {
            let mut trace = oer_esp32s31_phy::tracking::observation::ReadinessTrace::default();
            let mut polls = oer_hil_protocol::phy::HoldPolls::default();
            let mut uart = crate::hold::Uart;
            let mut observer = crate::hold::Observer::new(
                0,
                &mut polls,
                Some(&mut trace),
                &EmbassyClock,
                &mut uart,
            );
            #[cfg(oer_profile)]
            let timer_window = oer_espressif_executor::timer_observation::Window::begin();
            let result = maintain_concurrent_phy::<_, (), RomShortDelay, _>(
                &EmbassyClock,
                domain,
                &mut (),
                &mut observer,
            )
            .await;
            observer.report(0);
            #[cfg(oer_profile)]
            crate::hold::report_timer(timer_window);
            crate::hold::report_preparation(&trace, &polls);
            result.map_err(|error| {
                oer_esp32s31_board::rom::print_fmt(format_args!(
                    "OPEN_RADIO_HIL phy=FAIL step=154-maintain error={error:?}\n"
                ));
                RejectReason::Internal
            })?;
        }
        let foundation = clocked
            .reset_mac(domain)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-reset"))?
            .configure_foundation()
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-foundation"))?;
        let RegisteredIeee802154Operational {
            mut task,
            interrupts,
            route,
        } = RegisteredIeee802154Operational::new(foundation, member);
        let interrupts = interrupts.activate(&mut task);
        let mac = Ieee802154MacOwners::new(task, interrupts);
        let mut engine = match self.detached.take() {
            Some(detached) => detached.rebind(mac),
            None => Ieee802154Engine::place(
                mac,
                self.buffers.take().ok_or(RejectReason::Internal)?,
                ESP32S31_TX_POWER_LEVELS,
                Ieee802154PibDefaults::default(),
                permit,
            ),
        };
        engine.set_coexistence(Ieee802154Coexistence::Software(
            Ieee802154CoexPriorities::resolve(&domain.coex_pti_table()),
        ));
        engine.enable();
        engine.mac_init();
        let mut env = Environment::default();
        // SOURCE: firmware/peers/esp32c5-ieee802154/main/peer.c CFG,
        // tools/vendor/phy/src/reference.rs configuration(). The diagnostic
        // keeps RX auto-ACK disabled so no frame can make it transmit.
        engine.set_panid(0x1234);
        engine.set_short_address(0x0001);
        engine.set_extended_address([0x02, 0x4f, 0x45, 0x52, 0, 0, 0, 1]);
        engine.update_pib(&mut env, |pib| {
            pib.set_channel(permit);
            pib.set_power(power_dbm);
            pib.set_promiscuous(false);
            pib.set_rx_when_idle(true);
            pib.set_auto_ack_rx(false);
            pib.set_auto_ack_tx(false);
            pib.set_enhanced_ack_tx(false);
        });
        engine.receive(&mut env);
        if engine.state() != Ieee802154State::Rx {
            return Err(RejectReason::Internal);
        }
        ENGINE
            .with(|slot| {
                if slot.is_some() {
                    return Err(RejectReason::Internal);
                }
                *slot = Some(Running { engine, env });
                Ok(())
            })
            .ok_or(RejectReason::Internal)??;
        self.route = Some(route);
        oer_espressif_esp_hal_irq::enable(&self.irq)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-irq-on"))?;
        Ok(())
    }

    pub async fn exit(
        &mut self,
        domain: &mut RadioDomain<oer_espressif_esp_hal_radio::EspHal, ConcurrentPhy>,
    ) -> Result<()> {
        // Same P2 as the IRQ, with no yield until it is unrouted and stopped.
        oer_espressif_esp_hal_irq::disable(&self.irq);
        let mut running = ENGINE
            .with(Option::take)
            .flatten()
            .ok_or(RejectReason::Internal)?;
        running.engine.sleep(&mut running.env);
        running.recycle()?;
        running.engine.mac_deinit();
        let (detached, mac) = match running.engine.detach() {
            Ok(parts) => parts,
            Err(engine) => {
                // Preserve the failed owner; the enclosing radio is poisoned.
                ENGINE
                    .with(|slot| {
                        *slot = Some(Running {
                            engine,
                            env: running.env,
                        })
                    })
                    .ok_or(RejectReason::Internal)?;
                return Err(RejectReason::Internal);
            }
        };
        self.detached = Some(detached);
        let (mut task, interrupts) = mac.into_parts();
        let interrupts = interrupts.deactivate(&mut task);
        let (foundation, member) = self
            .route
            .take()
            .ok_or(RejectReason::Internal)?
            .into_foundation(task, interrupts)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-return"))?;
        let clocked = foundation.into_clocked();
        if !leave_ieee802154(domain, &clocked, member)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-leave"))?
        {
            return Err(RejectReason::Internal);
        }
        let powered = clocked
            .disable_clocks(domain)
            .map_err(|_| failed(c"OPEN_RADIO_HIL phy=FAIL step=154-clocks-off"))?;
        let (absent, _) = domain.leave_coex(self.coex.take().ok_or(RejectReason::Internal)?);
        self.cold = Some(
            powered
                .power_down(
                    domain,
                    self.power.take().ok_or(RejectReason::Internal)?,
                    &absent,
                )
                .map_err(|_| RejectReason::Internal)?,
        );
        self.absent = Some(absent);
        crate::sar2_checkpoint::observe(domain, crate::sar2_checkpoint::Point::BeforeRfClose)
            .await?;
        close_concurrent_rf::<_, (), RomShortDelay>(&EmbassyClock, domain, &mut ())
            .await
            .map_err(|_| RejectReason::Internal)?;
        Ok(())
    }
}

fn receive_permit(number: u8) -> Result<Ieee802154Permit> {
    // RX-only laboratory configuration, matching the reference's 21 dBm PIB
    // value without authorizing transmission. No TX/ACK path is exposed.
    // SOURCE: tools/vendor/phy/src/reference.rs CHANNEL/POWER_DBM.
    const RULES: &[Rule] = &[Rule::new(
        match FrequencySpan::new(2_400_000, 2_483_500) {
            Some(span) => span,
            None => panic!("RX span"),
        },
        2_000,
        2100,
        RuleFlags::NO_IR,
    )];
    let domain = RegulatoryDomain::new(
        Country::new(*b"ZZ").map_err(|_| RejectReason::Internal)?,
        RULES,
    )
    .map_err(|_| RejectReason::Internal)?;
    let channel = Ieee802154Channel::new(number).map_err(|_| RejectReason::InvalidConfiguration)?;
    let permitted = domain
        .permit(channel)
        .map_err(|_| RejectReason::InvalidConfiguration)?;
    let ceiling = permitted.tx_ceiling(PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: 2100,
        owner_cap_mbm: None,
    });
    Ieee802154Permit::new(permitted, ceiling, ESP32S31_TX_POWER_LEVELS)
        .map_err(|_| RejectReason::InvalidConfiguration)
}

#[derive(Default)]
struct Environment {
    received: heapless::Vec<Ieee802154RxSlot, { RX_BUFFER_COUNT + 1 }>,
    failed: bool,
}
impl Ieee802154Environment for Environment {
    fn now_micros(&mut self) -> u64 {
        EmbassyClock.now().as_micros()
    }
    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        _: &[u8; FRAME_SIZE],
        _: &Ieee802154FrameInfo,
    ) {
        self.failed |= self.received.push(slot).is_err();
    }
    fn receive_sfd_done(&mut self) {}
    fn transmit_done(&mut self, _: &[u8; FRAME_SIZE], _: Option<Ieee802154ReceivedAck<'_>>) {
        self.failed = true;
    }
    fn transmit_failed(&mut self, _: &[u8; FRAME_SIZE], _: Ieee802154TxError) {
        self.failed = true;
    }
    fn transmit_sfd_done(&mut self, _: &mut [u8; FRAME_SIZE]) {
        self.failed = true;
    }
    fn energy_detect_done(&mut self, _: i8) {
        self.failed = true;
    }
    fn cca_done(&mut self, _: bool) {
        self.failed = true;
    }
    fn ed_failed(&mut self, _: Ieee802154RxStatus) {
        self.failed = true;
    }
    fn receive_at_done(&mut self) {
        self.failed = true;
    }
    fn generate_enhanced_ack(
        &mut self,
        _: &[u8; FRAME_SIZE],
        _: &Ieee802154FrameInfo,
        _: &mut [u8; FRAME_SIZE],
    ) -> Ieee802154EnhancedAck {
        Ieee802154EnhancedAck::Refused
    }
}

fn failed(step: &'static core::ffi::CStr) -> RejectReason {
    oer_esp32s31_board::rom::line(step);
    RejectReason::Internal
}
