//! Image-local RX diagnostic assembly; no block runtime API.
use crate::radio::require_radio;
use oer_diagnostics::{DiagPath, Note};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::{CoexAbsent, Coexisting, CommonPower, Ieee802154Client, RadioDomain},
    ieee802154::{
        Ieee802154Cold,
        engine::{self, DetachedIeee802154Engine, Ieee802154Engine},
        ll::Ieee802154MacOwners,
        mac::Ieee802154InterruptOwner,
    },
    power::ChipClocks,
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
use oer_espressif_ieee802154_engine::{
    coex::{Ieee802154CoexPriorities, Ieee802154Coexistence},
    engine::{
        FRAME_SIZE, Ieee802154EngineBuffers, Ieee802154EnhancedAck, Ieee802154Environment,
        Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154SecurityFrameError,
        Ieee802154State, Ieee802154TxError, RX_BUFFER_COUNT,
    },
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
};
use oer_espressif_ieee802154_mac_values::{Ieee802154Channel, Ieee802154RxStatus};
use oer_espressif_interrupt_table::Silent;
use oer_hil_protocol::base::RejectReason;
use oer_memory::HandlerCell;
use oer_radio_regulatory::{
    Country, FrequencySpan, PowerLimits, RegulatoryDomain, Rule, RuleFlags,
};
use oer_time::Clock;
use static_cell::StaticCell;

type Result<T> = core::result::Result<T, RejectReason>;

// Only this image uses the hardware engine; retain its source observations
// without adding a record to unrelated staged images.
oer_espressif_staged_runtime::retained_static! {
    static EVENT_FAULT: oer_espressif_ieee802154_engine::engine::fault::FaultRecord;
}

fn reset_fault(fault: oer_espressif_ieee802154_engine::engine::Fault) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording(|_stop| {
        if let Some(mut record) = EVENT_FAULT.try_lock() {
            record.record(fault);
        }
    })
}

pub fn report_previous_fault() {
    if let Some(mut record) = EVENT_FAULT.try_lock() {
        let _reported = record.report(oer_esp32s31_hal::usb_serial_jtag::early_text);
    }
}

// The MAC's source is routed nowhere: the radio task (P2) polls the running
// engine's events and makes every other engine entry, so none preempts
// another. No borrow crosses await; P1 and P3 never enter this cell.
// HandlerCell rejects reentry without waiting or sharing a lock.
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

/// How often the radio task polls the running engine's events: well inside
/// the 4 ms a 127-byte PSDU lasts on air at the 2.4 GHz PHY's 250 kbit/s, so
/// a poll finds at most one new frame per receive slot.
const POLL_PERIOD: oer_time::Duration = oer_time::Duration::from_millis(1);

/// When the radio task polls the running engine next; `None` without one.
pub fn poll_deadline() -> Option<oer_time::Instant> {
    ENGINE
        .with(|running| running.is_some())
        .unwrap_or(false)
        .then(|| EmbassyClock.now().saturating_add(POLL_PERIOD))
}

/// Serve the running engine's events and return its delivered slots. A
/// slot the engine refuses fails the session, which its exit reports.
// Keep MAC event scratch off unrelated Wi-Fi calibration/hold call chains.
#[inline(never)]
pub fn poll() {
    let _entered = ENGINE.with(|running| {
        if let Some(Running { engine, mut env }) = running.take() {
            let engine = match engine.isr(&mut env) {
                Ok(engine) => engine,
                Err(quarantine) => reset_fault(quarantine.fault()),
            };
            *running = Some(Running { engine, env });
        }
        if let Some(running) = running.as_mut() {
            if running.recycle().is_err() {
                running.env.failed = true;
            }
        }
    });
}

pub struct Rx {
    cold: Option<Ieee802154Cold>,
    absent: Option<CoexAbsent<Ieee802154Client>>,
    coex: Option<Coexisting<Ieee802154Client>>,
    power: Option<CommonPower<Ieee802154Client>>,
    route: Option<RegisteredIeee802154OperationalRoute>,
    silent: Option<Silent<Ieee802154InterruptOwner>>,
    buffers: Option<oer_esp32s31_hal::dma_static::Placed<Ieee802154EngineBuffers>>,
    detached: Option<DetachedIeee802154Engine>,
}
impl Rx {
    pub fn new(
        partition: Ieee802154RadioPartition,
        absent: CoexAbsent<Ieee802154Client>,
        silent: Silent<Ieee802154InterruptOwner>,
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
            silent: Some(silent),
            buffers: Some(engine::claim_buffers().expect("one placed MAC storage claim")),
            detached: None,
        }
    }

    pub async fn enter(
        &mut self,
        phy_pilot: &mut crate::phy_pilot::Client,
        domain_slot: &mut Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
        fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
        clocks: &ChipClocks,
        channel: u8,
        power_dbm: i8,
    ) -> Result<()> {
        let permit = receive_permit(channel)?;
        let absent = self.absent.take().ok_or(RejectReason::Internal)?;
        let (coex, _) = require_radio(domain_slot.as_mut(), fault).enter_coex(absent);
        self.coex = Some(coex);
        let (powered, power) = self
            .cold
            .take()
            .ok_or(RejectReason::Internal)?
            .power_up(require_radio(domain_slot.as_mut(), fault), clocks)
            .map_err(|_| failed(oer_diagnostics::name!("154-power-up"; 32)))?;
        self.power = Some(power);
        crate::tx_rate_checkpoint::observe(
            require_radio(domain_slot.as_ref(), fault),
            crate::tx_rate_checkpoint::Point::AfterIeee802154CommonPower,
        )
        .await?;
        let clocked = powered
            .enable_clocks(require_radio(domain_slot.as_mut(), fault), clocks)
            .map_err(|_| failed(oer_diagnostics::name!("154-clocks-on"; 32)))?;
        let ready = phy_ready(require_radio(domain_slot.as_mut(), fault))
            .map_err(|_| failed(oer_diagnostics::name!("154-phy-ready"; 32)))?;
        let PhyClientAccess::Available(available) = ready.client::<Ieee802154PhyMembership>()
        else {
            return Err(RejectReason::Internal);
        };
        let joined = join_ieee802154(available, &clocked, &crate::PHY_CLOCK).map_err(|error| {
            crate::transport::CONSOLE
                .note_immediately(&Note::ieee802154_join_fail(&DiagPath::of(&error)));
            RejectReason::Internal
        })?;
        crate::transport::CONSOLE
            .note_reliably(&Note::joined(&DiagPath::of(&joined.acquired)))
            .await;
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
                &crate::PHY_CLOCK,
                &mut uart,
            );
            #[cfg(oer_profile)]
            let timer_window = oer_espressif_executor::timer_observation::Window::begin();
            let result = {
                let mut platform = ();
                let domain = require_radio(domain_slot.take(), fault);
                let observer = &mut observer;
                let mut maintain = core::pin::pin!(crate::completion::construct(|| maintain_concurrent_phy::<_, (), RomShortDelay, _>(
                    &crate::PHY_CLOCK,
                    domain,
                    &mut platform,
                    observer,
                    phy_pilot,
                )));
                crate::completion::run(maintain.as_mut(), domain_slot, crate::completion::restore).await
            };
            observer.report(0).await;
            #[cfg(oer_profile)]
            crate::hold::report_timer(timer_window).await;
            crate::hold::report_preparation(&trace, &polls).await;
            result.map_err(|error| {
                crate::transport::CONSOLE
                    .note_immediately(&Note::ieee802154_maintain_fail(&DiagPath::of(&error)));
                RejectReason::Internal
            })?;
        }
        let foundation = clocked
            .reset_mac(require_radio(domain_slot.as_mut(), fault))
            .map_err(|_| failed(oer_diagnostics::name!("154-reset"; 32)))?
            .configure_foundation()
            .map_err(|_| failed(oer_diagnostics::name!("154-foundation"; 32)))?;
        let RegisteredIeee802154Operational {
            task,
            interrupts,
            route,
        } = RegisteredIeee802154Operational::new(foundation, member);
        let silent = self.silent.take().ok_or(RejectReason::Internal)?;
        let (startup, interrupts) = interrupts.arm(task);
        let mac = Ieee802154MacOwners::new(startup, interrupts, silent);
        let mut engine = match self.detached.take() {
            Some(detached) => detached.rebind(mac),
            None => Ieee802154Engine::place(
                mac,
                self.buffers.take().ok_or(RejectReason::Internal)?,
                ESP32S31_TX_POWER_LEVELS,
                Ieee802154PibDefaults::default(),
                permit,
            )
            .map_err(|_| failed(oer_diagnostics::name!("154-placement"; 32)))?,
        };
        engine.set_coexistence(Ieee802154Coexistence::Software(
            Ieee802154CoexPriorities::resolve(
                &require_radio(domain_slot.as_mut(), fault).coex_pti_table(),
            ),
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
        Ok(())
    }

    pub async fn exit(
        &mut self,
        domain_slot: &mut Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
        fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
    ) -> Result<()> {
        // Same P2 task as the polls, with no yield until the engine stopped.
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
        let (mut task, interrupts, silent) = mac.into_parts();
        self.silent = Some(silent);
        let interrupts = interrupts.deactivate(&mut task);
        let (foundation, member) = self
            .route
            .take()
            .ok_or(RejectReason::Internal)?
            .into_foundation(task, interrupts)
            .map_err(|_| failed(oer_diagnostics::name!("154-return"; 32)))?;
        let clocked = foundation.into_clocked();
        if !leave_ieee802154(require_radio(domain_slot.as_mut(), fault), &clocked, member)
            .map_err(|_| failed(oer_diagnostics::name!("154-leave"; 32)))?
        {
            return Err(RejectReason::Internal);
        }
        let powered = clocked
            .disable_clocks(require_radio(domain_slot.as_mut(), fault))
            .map_err(|_| failed(oer_diagnostics::name!("154-clocks-off"; 32)))?;
        let (absent, _) = require_radio(domain_slot.as_mut(), fault)
            .leave_coex(self.coex.take().ok_or(RejectReason::Internal)?);
        self.cold = Some(
            powered
                .power_down(
                    require_radio(domain_slot.as_mut(), fault),
                    self.power.take().ok_or(RejectReason::Internal)?,
                    &absent,
                )
                .map_err(|_| RejectReason::Internal)?,
        );
        self.absent = Some(absent);
        crate::sar2_checkpoint::observe(
            domain_slot,
            fault,
            crate::sar2_checkpoint::Point::BeforeRfClose,
        )
        .await?;
        {
            let mut platform = ();
            let domain = require_radio(domain_slot.take(), fault);
            let mut close = core::pin::pin!(crate::completion::construct(|| close_concurrent_rf::<_, (), RomShortDelay>(
                &crate::PHY_CLOCK,
                domain,
                &mut platform,
            )));
            crate::completion::run(close.as_mut(), domain_slot, crate::completion::restore).await
        }
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
    fn enhanced_ack_security_failed(&mut self, _error: Ieee802154SecurityFrameError) {
        self.failed = true;
    }

    fn coexistence_break(&mut self) {
        // S31 libcoexist hal_ieee802154_coex_break_notify has no effects.
        // SOURCE: BLOB_LIBCOEX_IEEE802154_HANDOFF; no RF grant is released.
    }

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

fn failed<const B: usize>(step: oer_diagnostics::Name<B>) -> RejectReason {
    crate::transport::CONSOLE.note_immediately(&Note::phy_fail(step));
    RejectReason::Internal
}
