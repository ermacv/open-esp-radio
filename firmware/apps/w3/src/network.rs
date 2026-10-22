//! The owned Embassy/Xarxa device, DHCP and bounded W3 application.
use crate::{
    W3Config,
    station::{AUTHENTICATED, DEAUTH, DEPARTING, LAST_BEACON_MS},
};
use core::{
    cell::{Cell, RefCell},
    sync::atomic::Ordering,
};
use embassy_futures::{
    join::join3,
    select::{Either, select},
    yield_now,
};
use oer_embassy_net::{
    NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources, OwnedNetworkDevice,
    OwnedNetworkRunner,
};
use oer_embassy_time::EmbassyClock;
use oer_firmware_apps_w3::{
    echo::ECHO_COUNT,
    icmp,
    observation::Observations,
    report::{Lease, Report},
};
use oer_ieee80211_runtime::station_service::*;
use oer_ieee80211_sta::machine::{StaCommandCredits, StaRequest};
use oer_ieee80211_wire::{data::EthernetFrameParts, qos::WmmUserPriority};
use oer_radio_port::PoolPath;
use oer_time::{Clock, Duration, Timer};
use owned_embassy_net::{
    Stack, StackStorage,
    iface::{Iface, dhcpv4::DhcpConfig},
    raw::RawSocket,
    wire::{IpProtocol, IpVersion},
};
use static_cell::{ConstStaticCell, StaticCell};
use xarxa_driver::{PacketPool, PacketPoolStorage};

const RX: usize = oer_firmware_apps_w3::policy::RX_DEPTH;
const TX: usize = 4;
type Endpoint = OwnedEndpointResources<NoopRawMutex, RX, TX>;
type Device = OwnedNetworkDevice<'static, NoopRawMutex, RX, TX>;
type Runner = OwnedNetworkRunner<'static, NoopRawMutex, RX, TX>;
static ENDPOINT: StaticCell<Endpoint> = StaticCell::new();
static DEVICE: StaticCell<Device> = StaticCell::new();
static STACK: StaticCell<StackStorage<'static>> = StaticCell::new();
static GENERAL_STORAGE: ConstStaticCell<PacketPoolStorage<16>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static GENERAL_POOL: StaticCell<PacketPool<16>> = StaticCell::new();
static RX_STORAGE: ConstStaticCell<PacketPoolStorage<8>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static RX_POOL: StaticCell<PacketPool<8>> = StaticCell::new();

#[allow(clippy::too_many_arguments)]
pub async fn workload<'id, Path: PoolPath>(
    config: W3Config,
    address: [u8; 6],
    seed: u64,
    credits: StaCommandCredits<'id, Path>,
    mut commands: MsduSender<'_, ServiceCommand<'id, crate::radio::Caps, Path>, 2>,
    tx: MsduSender<'_, TxMsdu, 2>,
    rx: MsduReceiver<'_, EthernetFrame, RX>,
    link: &Cell<bool>,
    disconnected: &Cell<bool>,
    failed: &Cell<bool>,
) -> Report {
    let general = GENERAL_POOL
        .init_with(|| PacketPool::new(GENERAL_STORAGE.take()))
        .allocator();
    let receive = RX_POOL
        .init_with(|| PacketPool::new(RX_STORAGE.take()))
        .allocator();
    let (device, radio) =
        ENDPOINT
            .init_with(Endpoint::new)
            .split(NetworkInterfaceId::new(0), address, receive);
    let (stack, mut runner) = Stack::new(STACK.init_with(StackStorage::new), seed, general);
    runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 8));
    let iface = stack
        .add_iface_borrowed(DEVICE.init(device))
        .expect("one station IP interface");
    iface
        .set_dhcpv4(Some(DhcpConfig::default()))
        .expect("Ethernet DHCP");
    let socket = RawSocket::new(stack, Some(IpVersion::V4), Some(IpProtocol::Icmp))
        .expect("one W3 ICMP socket");
    let identifier = seed as u16;
    let observations = RefCell::new(Observations::new(identifier));
    let application = application(
        config,
        address,
        credits,
        &mut commands,
        iface,
        &socket,
        &observations,
        link,
        disconnected,
        failed,
    );
    match select(
        application,
        join3(
            bridge(radio, tx, rx, &observations, link),
            runner.run(),
            drain(&socket),
        ),
    )
    .await
    {
        Either::First(report) => report,
        Either::Second(_) => unreachable!("permanent network runners do not return"),
    }
}
async fn drain(socket: &RawSocket<'_>) -> ! {
    loop {
        let _ = socket.recv_packet().await;
        yield_now().await;
    }
}
async fn bridge(
    radio: Runner,
    mut tx: MsduSender<'_, TxMsdu, 2>,
    mut rx: MsduReceiver<'_, EthernetFrame, RX>,
    observations: &RefCell<Observations>,
    link: &Cell<bool>,
) -> ! {
    let mut state = false;
    loop {
        if state != link.get() {
            state = link.get();
            radio.link_controller().set_link_up(state);
        }
        while let Some(frame) = rx.try_receive() {
            let p = frame.parts();
            observations.borrow_mut().observe(
                p.ether_type,
                p.payload,
                false,
                EmbassyClock.now().as_micros(),
            );
            if radio
                .rx_publisher()
                .try_send_parts(p.destination, p.source, p.ether_type, p.payload)
                .is_err()
            {
                let mut o = observations.borrow_mut();
                o.queue_drops = o.queue_drops.saturating_add(1);
            }
        }
        if let Some(packet) = radio.try_receive_tx() {
            let bytes = packet.ethernet();
            if let Some((header, payload)) = bytes.split_first_chunk::<14>() {
                let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = *header;
                let ether_type = u16::from_be_bytes([e0, e1]);
                match EthernetFrame::from_parts(EthernetFrameParts {
                    destination: [d0, d1, d2, d3, d4, d5],
                    source: [s0, s1, s2, s3, s4, s5],
                    ether_type,
                    payload,
                }) {
                    Ok(frame) => {
                        let permit = tx.ready().await;
                        observations.borrow_mut().observe(
                            ether_type,
                            payload,
                            true,
                            EmbassyClock.now().as_micros(),
                        );
                        permit.send(TxMsdu {
                            priority: WmmUserPriority::new(0).expect("best effort priority"),
                            frame,
                        });
                    }
                    Err(_) => {
                        let mut o = observations.borrow_mut();
                        o.queue_drops = o.queue_drops.saturating_add(1);
                    }
                }
            }
            drop(packet);
            yield_now().await;
        } else {
            // The timer also propagates a link notice when no frame is queued.
            let _ = select(
                rx.ready(),
                select(
                    radio.wait_tx_publication(),
                    EmbassyClock.wait_for(Duration::from_millis(1)),
                ),
            )
            .await;
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn application<'id, Path: PoolPath>(
    config: W3Config,
    address: [u8; 6],
    credits: StaCommandCredits<'id, Path>,
    commands: &mut MsduSender<'_, ServiceCommand<'id, crate::radio::Caps, Path>, 2>,
    iface: Iface<'_>,
    socket: &RawSocket<'_>,
    observations: &RefCell<Observations>,
    link: &Cell<bool>,
    disconnected: &Cell<bool>,
    failed: &Cell<bool>,
) -> Report {
    let mut report = Report::new(config.channel, address, config.stand);
    let _ = commands.try_send(ServiceCommand::Station(StaRequest::new(1, credits.connect)));
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(30))
        .expect("W3 monotonic deadline");
    while !link.get() && !disconnected.get() && !failed.get() && EmbassyClock.now() < deadline {
        let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
    }
    report.authenticated = AUTHENTICATED.load(Ordering::Relaxed);
    report.associated = link.get();
    LAST_BEACON_MS.store(
        (EmbassyClock.now().as_micros() / 1000) as u32,
        Ordering::Relaxed,
    );
    if link.get() {
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(35))
            .expect("W3 monotonic deadline");
        loop {
            if let Some(lease) = iface.dhcpv4_lease()
                && let Some(router) = lease.router
            {
                observations.borrow_mut().lease = Some(Lease {
                    address: lease.address.address(),
                    router,
                    prefix: lease.address.prefix_len(),
                });
                break;
            }
            if !healthy(link, failed) || EmbassyClock.now() >= deadline {
                break;
            }
            let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
        }
    }
    let lease = observations.borrow().lease;
    if let Some(lease) = lease {
        // The host observes the lease and starts its capture in this window.
        let mut next = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(10))
            .expect("W3 monotonic deadline");
        let failure_deadline = next
            .checked_add(Duration::from_secs(140))
            .expect("W3 ping deadline");
        for sequence in 1..=ECHO_COUNT {
            while EmbassyClock.now() < next && healthy(link, failed) {
                let wake = EmbassyClock
                    .now()
                    .saturating_add(Duration::from_millis(10))
                    .min(next);
                let _ = EmbassyClock.wait_until(wake).await;
            }
            if !healthy(link, failed) {
                break;
            }
            let packet = icmp::request(
                lease.address,
                lease.router,
                observations.borrow().identifier(),
                sequence,
            );
            if !matches!(
                select(
                    socket.send(&packet),
                    EmbassyClock.wait_for(Duration::from_millis(250))
                )
                .await,
                Either::First(Ok(()))
            ) {
                failed.set(true);
                break;
            }
            next = next
                .checked_add(Duration::from_secs(1))
                .expect("W3 ping schedule");
        }
        while !observations.borrow().complete()
            && healthy(link, failed)
            && EmbassyClock.now() < failure_deadline
        {
            let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
        }
        // Retire the last queued data transmission before requesting departure.
        if healthy(link, failed) {
            let _ = EmbassyClock.wait_for(Duration::from_secs(2)).await;
        }
    }
    observations.borrow().report(&mut report);
    if link.get() {
        DEPARTING.store(true, Ordering::Relaxed);
        let _ = commands.try_send(ServiceCommand::Station(StaRequest::new(
            2,
            credits.disconnect,
        )));
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(10))
            .expect("W3 monotonic deadline");
        while !disconnected.get() && EmbassyClock.now() < deadline {
            let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
        }
    }
    report.deauth_sent = DEAUTH.load(Ordering::Relaxed);
    let _ = commands.try_send(ServiceCommand::Stop);
    report
}

fn healthy(link: &Cell<bool>, failed: &Cell<bool>) -> bool {
    let last = LAST_BEACON_MS.load(Ordering::Relaxed);
    if oer_firmware_apps_w3::beacon::lost(last, (EmbassyClock.now().as_micros() / 1000) as u32) {
        failed.set(true);
    }
    link.get() && !failed.get()
}
