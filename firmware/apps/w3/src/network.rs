//! The owned Embassy/Xarxa device, DHCP and bounded W3 application.
use crate::{
    W3Config,
    radio::{AUTHENTICATED, DEAUTH, DEPARTING, LAST_BEACON_MS},
};
use core::sync::atomic::AtomicBool;
use core::{cell::RefCell, sync::atomic::Ordering};
use embassy_futures::{
    join::join3,
    select::{Either, select},
    yield_now,
};
use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources, OwnedNetworkDevice};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_wifi::{Control, Network};
use oer_firmware_apps_w3::{
    icmp,
    observation::Observations,
    report::{Lease, Report},
};
use oer_time::{Clock, Duration, Timer};
use owned_embassy_net::{
    Stack, StackStorage,
    iface::{Iface, dhcpv4::DhcpConfig},
    raw::RawSocket,
    wire::{IpProtocol, IpVersion},
};
use static_cell::{ConstStaticCell, StaticCell};
use xarxa_driver::{PacketPool, PacketPoolStorage};

const RX: usize = oer_esp32s31_wifi::RX_DEPTH;
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
pub fn workload<'a>(
    config: W3Config,
    address: [u8; 6],
    seed: u64,
    commands: Control<'a>,
    net: Network<'a>,
    link: &'a AtomicBool,
    disconnected: &'a AtomicBool,
    failed: &'a AtomicBool,
) -> impl core::future::Future<Output = Report> + 'a {
    WORKLOAD.take().future(|| {
        workload_future(
            config,
            address,
            seed,
            commands,
            net,
            link,
            disconnected,
            failed,
        )
    })
}

type Workload<'a> = impl core::future::Future<Output = Report>;
static WORKLOAD: ConstStaticCell<oer_memory::StaticPlace<Workload<'static>>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

#[allow(clippy::too_many_arguments)]
#[define_opaque(Workload)]
#[inline(never)]
fn workload_future<'a>(
    config: W3Config,
    address: [u8; 6],
    seed: u64,
    mut commands: Control<'a>,
    net: Network<'a>,
    link: &'a AtomicBool,
    disconnected: &'a AtomicBool,
    failed: &'a AtomicBool,
) -> Workload<'a> {
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
            &mut commands,
            iface,
            &socket,
            &observations,
            link,
            disconnected,
            failed,
        );
        match select(
            // Poll event producers before the application's value predicates.
            // They share this task's waker, so a bridge/stack event also polls
            // the application after the corresponding state update.
            join3(
                async {
                    match oer_firmware_apps_w3::bridge::run(
                        radio,
                        net,
                        &observations,
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
            application,
        )
        .await
        {
            Either::Second(mut report) => {
                report.stack = Some(stack.statistics());
                report
            }
            Either::First((_, never, _)) => match never {},
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
async fn application(
    config: W3Config,
    address: [u8; 6],
    commands: &mut Control<'_>,
    iface: Iface<'_, Device>,
    socket: &RawSocket<'_, Device>,
    observations: &RefCell<Observations>,
    link: &AtomicBool,
    disconnected: &AtomicBool,
    failed: &AtomicBool,
) -> Report {
    let mut report = Report::new(config.channel, address, config.stand);
    let _ = commands.try_connect(1);
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(30))
        .expect("W3 monotonic deadline");
    wait_for(deadline, || {
        link.load(Ordering::Relaxed)
            || disconnected.load(Ordering::Relaxed)
            || failed.load(Ordering::Relaxed)
    })
    .await;
    report.authenticated = AUTHENTICATED.load(Ordering::Relaxed);
    report.associated = link.load(Ordering::Relaxed);
    LAST_BEACON_MS.store(
        (EmbassyClock.now().as_micros() / 1000) as u32,
        Ordering::Relaxed,
    );
    if link.load(Ordering::Relaxed) {
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(35))
            .expect("W3 monotonic deadline");
        wait_healthy(deadline, link, failed, || {
            if let Some(lease) = iface.dhcpv4_lease()
                && let Some(router) = lease.router
            {
                observations.borrow_mut().lease = Some(Lease {
                    address: lease.address.address(),
                    router,
                    prefix: lease.address.prefix_len(),
                });
                true
            } else {
                false
            }
        })
        .await;
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
            async |next| {
                wait_healthy(next, link, failed, || false).await;
                healthy(link, failed)
            },
        )
        .await;
        wait_healthy(failure_deadline, link, failed, || {
            observations.borrow().complete()
        })
        .await;
        // Retire the last queued data transmission before requesting departure.
        if healthy(link, failed) {
            let _ = EmbassyClock.wait_for(Duration::from_secs(2)).await;
        }
    }
    observations.borrow().report(&mut report);
    if link.load(Ordering::Relaxed) {
        DEPARTING.store(true, Ordering::Relaxed);
        let _ = commands.try_disconnect(2);
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(10))
            .expect("W3 monotonic deadline");
        wait_for(deadline, || disconnected.load(Ordering::Relaxed)).await;
    }
    report.deauth_sent = DEAUTH.load(Ordering::Relaxed);
    commands.stop();
    report
}

// These predicates are polled in the same task, after the bridge, IP runner
// and station runner. Their RX/TX/link/stack wakers drive the predicates too;
// the only timers here are application deadlines, including beacon loss.
async fn wait_for(deadline: oer_time::Instant, mut ready: impl FnMut() -> bool) {
    let _ = select(
        core::future::poll_fn(|_| {
            if ready() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        }),
        EmbassyClock.wait_until(deadline),
    )
    .await;
}

async fn wait_healthy(
    deadline: oer_time::Instant,
    link: &AtomicBool,
    failed: &AtomicBool,
    mut ready: impl FnMut() -> bool,
) {
    while !ready() && healthy(link, failed) && EmbassyClock.now() < deadline {
        let now = EmbassyClock.now();
        let elapsed =
            ((now.as_micros() / 1000) as u32).wrapping_sub(LAST_BEACON_MS.load(Ordering::Relaxed));
        let beacon_deadline =
            now.saturating_add(Duration::from_millis(6000u32.saturating_sub(elapsed)));
        wait_for(deadline.min(beacon_deadline), || {
            ready() || !healthy(link, failed)
        })
        .await;
    }
}

fn healthy(link: &AtomicBool, failed: &AtomicBool) -> bool {
    let last = LAST_BEACON_MS.load(Ordering::Relaxed);
    if oer_firmware_apps_w3::beacon::lost(last, (EmbassyClock.now().as_micros() / 1000) as u32) {
        failed.store(true, Ordering::Relaxed);
    }
    link.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed)
}
