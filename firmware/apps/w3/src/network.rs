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
use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources, OwnedNetworkDevice};
use oer_embassy_time::EmbassyClock;
use oer_firmware_apps_w3::{
    icmp,
    observation::Observations,
    report::{Lease, Report},
};
use oer_ieee80211_runtime::station_service::*;
use oer_ieee80211_sta::data::{EthernetFrame, TxMsdu};
use oer_ieee80211_sta::machine::{StaCommandCredits, StaRequest};
use oer_memory::{WakingSpscReceiver, WakingSpscSender};
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
type Endpoint = OwnedEndpointResources<oer_espressif_executor::ThreadContext<0>, RX, TX>;
type Device = OwnedNetworkDevice<'static, oer_espressif_executor::ThreadContext<0>, RX, TX>;
static ENDPOINT: StaticCell<Endpoint> = StaticCell::new();
static DEVICE: StaticCell<Device> = StaticCell::new();
static STACK: StaticCell<StackStorage<'static, Device>> = StaticCell::new();
static GENERAL_STORAGE: ConstStaticCell<PacketPoolStorage<16>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static GENERAL_POOL: StaticCell<PacketPool<16>> = StaticCell::new();
static RX_STORAGE: ConstStaticCell<PacketPoolStorage<8>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static RX_POOL: StaticCell<PacketPool<8>> = StaticCell::new();

#[allow(clippy::too_many_arguments)]
pub fn workload<'a, 'id: 'a, Path: PoolPath + 'a>(
    config: W3Config,
    address: [u8; 6],
    seed: u64,
    credits: StaCommandCredits<'id, Path>,
    commands: WakingSpscSender<'a, ServiceCommand<'id, crate::radio::Caps, Path>, 2>,
    tx: WakingSpscSender<'a, TxMsdu, 2>,
    rx: WakingSpscReceiver<'a, EthernetFrame, RX>,
    link: &'a Cell<bool>,
    disconnected: &'a Cell<bool>,
    failed: &'a Cell<bool>,
) -> impl core::future::Future<Output = Report> + 'a {
    WORKLOAD.take().future(|| {
        workload_future(
            config,
            address,
            seed,
            credits,
            commands,
            tx,
            rx,
            link,
            disconnected,
            failed,
        )
    })
}

type Workload<'a, 'id: 'a, Path: PoolPath + 'a> = impl core::future::Future<Output = Report>;
type CommandPath = oer_radio_port::Child<oer_radio_port::Root<crate::radio::slot::Id>, 8>;
static WORKLOAD: ConstStaticCell<oer_memory::StaticPlace<Workload<'static, 'static, CommandPath>>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

#[allow(clippy::too_many_arguments)]
#[define_opaque(Workload)]
#[inline(never)]
fn workload_future<'a, 'id: 'a, Path: PoolPath + 'a>(
    config: W3Config,
    address: [u8; 6],
    seed: u64,
    credits: StaCommandCredits<'id, Path>,
    mut commands: WakingSpscSender<'a, ServiceCommand<'id, crate::radio::Caps, Path>, 2>,
    tx: WakingSpscSender<'a, TxMsdu, 2>,
    rx: WakingSpscReceiver<'a, EthernetFrame, RX>,
    link: &'a Cell<bool>,
    disconnected: &'a Cell<bool>,
    failed: &'a Cell<bool>,
) -> Workload<'a, 'id, Path> {
    async move {
        let general = GENERAL_POOL
            .init_with(|| PacketPool::new(GENERAL_STORAGE.take()))
            .allocator();
        let receive = RX_POOL
            .init_with(|| PacketPool::new(RX_STORAGE.take()))
            .allocator();
        let (device, radio) = ENDPOINT
            .init_with(Endpoint::new)
            .split(NetworkInterfaceId::new(0), address, receive)
            .unwrap_or_else(|error| match error {
                oer_embassy_net::EndpointSplitError::AlreadySplit => {
                    crate::fail(oer_diagnostics::name!("network-endpoint-already-split"; 32))
                }
                oer_embassy_net::EndpointSplitError::RxWaiterUnavailable => {
                    crate::fail(oer_diagnostics::name!("network-rx-waiter-unavailable"; 32))
                }
            });
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
                async {
                    match oer_firmware_apps_w3::bridge::run(
                        radio,
                        tx,
                        rx,
                        &observations,
                        link,
                        &EmbassyClock,
                    )
                    .await
                    {
                        Ok(never) => match never {},
                        Err(_) => crate::fail(
                            oer_diagnostics::name!("network-context-access-refused"; 32),
                        ),
                    }
                },
                runner.run(),
                drain(&socket),
            ),
        )
        .await
        {
            Either::First(mut report) => {
                report.stack = Some(stack.statistics());
                report
            }
            Either::Second(_) => unreachable!("permanent network runners do not return"),
        }
    }
}
async fn drain(socket: &RawSocket<'_, Device>) -> ! {
    loop {
        let _ = socket.recv_packet().await;
        yield_now().await;
    }
}
#[allow(clippy::too_many_arguments)]
async fn application<'id, Path: PoolPath>(
    config: W3Config,
    address: [u8; 6],
    credits: StaCommandCredits<'id, Path>,
    commands: &mut WakingSpscSender<'_, ServiceCommand<'id, crate::radio::Caps, Path>, 2>,
    iface: Iface<'_, Device>,
    socket: &RawSocket<'_, Device>,
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
        let next = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(10))
            .expect("W3 monotonic deadline");
        let failure_deadline = next
            .checked_add(Duration::from_secs(140))
            .expect("W3 ping deadline");
        report.probes = oer_firmware_apps_w3::probe::run(
            &EmbassyClock,
            next,
            async |sequence| {
                let packet = icmp::request(
                    lease.address,
                    lease.router,
                    observations.borrow().identifier(),
                    sequence,
                );
                socket.send(&packet).await
            },
            || healthy(link, failed),
        )
        .await;
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
