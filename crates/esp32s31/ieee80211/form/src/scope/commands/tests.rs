extern crate std;
mod payload;
use super::*;
use core::{future::Future, pin::pin, task::Waker};
use oer_ieee80211_lower_mac::{ChannelWidth, LowerMacCapabilities, ReceiveFilter};
use oer_radio_port::branded;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Wake,
    vec::Vec,
};

struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Interface;
    const CAPS: LowerMacCapabilities =
        oer_esp32s31_ieee80211::lower_mac::esp32s31_lower_mac_capabilities(2048);
}
fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the test prepublished every awaited command"),
    }
}
fn ap_filter() -> SupportedFilter<Caps, AccessPoint> {
    SupportedFilter::new(ReceiveFilter::NONE).unwrap()
}

#[test]
fn full_fifo_retains_whole_in_flight_context_and_multiple_bodies_until_terminal() {
    use core::convert::Infallible;
    use oer_ieee80211_lower_mac::{SingleOutcome, WithdrawCause};
    let contexts = [
        std::string::String::from("first"),
        std::string::String::from("second"),
    ];
    ready(branded::<CommandPools<2, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let mut terminals = r.terminals;
        let mut store =
            CommandStorage::<Caps, usize, usize, &str, payload::Mpdu, 2, 2, 4, 15>::new(r.bridge);
        let (setup, mut rx) = store.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        let SetupCommand::Enable(enable) = rx.next().await else {
            panic!("enable");
        };
        let mut rx = rx.enabled(enable);
        tx.close_gate(r.gate);
        let PhaseCommand::CloseGate(closing) = rx.next().await else {
            panic!("close");
        };
        let closed = tx.closed(rx.gate_barrier(closing));
        let open = tx.open_gate(closed);
        let mut identity = Vec::new();
        for (index, (ticket, core_ticket)) in r.tickets.into_iter().zip(r.core.tickets).enumerate()
        {
            let operation = payload::admit(contexts[index].as_str(), ticket, index as u8);
            identity.push((
                operation.payload().bodies[0].as_ptr(),
                operation.payload().bodies[1].as_ptr(),
            ));
            tx.submit(operation, TxAdmission::Open(core_ticket));
        }
        tx.close_gate(open);
        tx.configure(Configuration::Station {
            permission: r.station.configure.with_context(11),
            receive: StationReceive::Off,
        });
        tx.configure(Configuration::AccessPoint {
            permission: r.access_point.configure.with_context(22),
            receive: ap_filter(),
        });
        tx.sample_tsf(Sampling::Station(r.station.tsf));
        tx.sample_tsf(Sampling::AccessPoint(r.access_point.tsf));
        for slot in r.installs {
            tx.install_pairwise(slot, Ccmp128Key::new(&zeroize::Zeroizing::new([1; 16])));
        }
        for slot in r.removes {
            tx.remove_pairwise(slot);
        }
        for slot in r.groups {
            tx.mutate_group(slot, Ccmp128Key::new(&zeroize::Zeroizing::new([2; 16])));
        }
        let tx = tx.disable();
        // All 15 distinct admissions coexist before the consumer advances.
        let mut frames = 0;
        let mut commands = 0;
        loop {
            commands += 1;
            match rx.next().await {
                PhaseCommand::Submit(mut submission) => {
                    let view = submission.operation.execute();
                    assert_eq!(view.header, &[frames as u8; 24]);
                    assert_eq!(
                        (view.bodies[0].as_ptr(), view.bodies[1].as_ptr()),
                        identity[frames]
                    );
                    assert_eq!(view.sn, 10 + frames as u16);
                    assert_eq!(view.pn, 500 + frames as u64);
                    let terminal =
                        submission
                            .operation
                            .complete(SingleOutcome::<(), Infallible>::Withdrawn(
                                WithdrawCause::GateClosed,
                            ));
                    let original = terminal.into_parts(&mut terminals);
                    assert_eq!(original.context.as_ptr(), contexts[frames].as_ptr());
                    assert_eq!(
                        (
                            original.payload.bodies[0].as_ptr(),
                            original.payload.bodies[1].as_ptr()
                        ),
                        identity[frames]
                    );
                    assert_eq!(
                        (original.payload.sn, original.payload.pn),
                        (10 + frames as u16, 500 + frames as u64)
                    );
                    frames += 1;
                }
                PhaseCommand::Disable(disable) => {
                    let (_rx, reply) = rx.disabled(disable);
                    let _setup = tx.finished(reply);
                    break;
                }
                _command => {}
            }
        }
        assert_eq!((commands, frames), (15, 2));
    }));
}
type Storage<'id, const K: usize, const P: usize> =
    CommandStorage<'id, Caps, usize, usize, (), (), 0, K, 4, P>;

#[test]
fn every_minted_non_tx_credit_and_disable_fit_at_once_in_fifo_order() {
    ready(branded::<CommandPools<0, 2>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store = Storage::<2, 13>::new(r.bridge);
        let (setup, mut rx) = store.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        let SetupCommand::Enable(enable) = rx.next().await else {
            panic!("enable");
        };
        let mut rx = rx.enabled(enable);
        tx.close_gate(r.gate);
        let PhaseCommand::CloseGate(closing) = rx.next().await else {
            panic!("close");
        };
        let barrier = rx.gate_barrier(closing);
        let closed = tx.closed(barrier);
        let open = tx.open_gate(closed);
        // Open and Close coexist with every other class, including the final
        // Disable. No pop can hide a capacity shortage in this interval.
        tx.close_gate(open);
        tx.configure(Configuration::Station {
            permission: r.station.configure.with_context(11),
            receive: StationReceive::Off,
        });
        tx.configure(Configuration::AccessPoint {
            permission: r.access_point.configure.with_context(22),
            receive: ap_filter(),
        });
        tx.sample_tsf(Sampling::Station(r.station.tsf));
        tx.sample_tsf(Sampling::AccessPoint(r.access_point.tsf));
        for permission in r.installs {
            tx.install_pairwise(
                permission,
                Ccmp128Key::new(&zeroize::Zeroizing::new([1; 16])),
            );
        }
        for permission in r.removes {
            tx.remove_pairwise(permission);
        }
        for permission in r.groups {
            tx.mutate_group(
                permission,
                Ccmp128Key::new(&zeroize::Zeroizing::new([2; 16])),
            );
        }
        let tx = tx.disable();
        let mut order = Vec::new();
        let mut contexts = Vec::new();
        loop {
            match rx.next().await {
                PhaseCommand::OpenGate => order.push("open"),
                PhaseCommand::CloseGate(_) => order.push("close"),
                PhaseCommand::Configure(Configuration::Station { permission, .. }) => {
                    order.push("sta-config");
                    contexts.push(permission.into_parts().1);
                }
                PhaseCommand::Configure(Configuration::AccessPoint { permission, .. }) => {
                    order.push("ap-config");
                    contexts.push(permission.into_parts().1);
                }
                PhaseCommand::SampleTsf(Sampling::Station(_)) => order.push("sta-tsf"),
                PhaseCommand::SampleTsf(Sampling::AccessPoint(_)) => order.push("ap-tsf"),
                PhaseCommand::InstallPairwise { .. } => order.push("install"),
                PhaseCommand::RemovePairwise(_) => order.push("remove"),
                PhaseCommand::MutateGroup { .. } => order.push("group"),
                PhaseCommand::Disable(disable) => {
                    order.push("disable");
                    let (_rx, reply) = rx.disabled(disable);
                    let _setup = tx.finished(reply);
                    break;
                }
                PhaseCommand::Submit(_) => panic!("zero TX tickets"),
            }
        }
        assert_eq!(contexts, [11, 22]);
        assert_eq!(
            order,
            [
                "open",
                "close",
                "sta-config",
                "ap-config",
                "sta-tsf",
                "ap-tsf",
                "install",
                "install",
                "remove",
                "remove",
                "group",
                "group",
                "disable"
            ]
        );
    }));
}

#[test]
fn setup_configuration_and_retune_precede_enable_and_phase_commands() {
    ready(branded::<CommandPools<0, 0>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store = Storage::<0, 9>::new(r.bridge);
        let (mut tx, mut rx) = store.split(r.issuer, r.enable, r.disable);
        tx.configure(Configuration::Station {
            permission: r.station.configure.with_context(31),
            receive: StationReceive::OtherBssManagement,
        });
        let channel = Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap();
        tx.retune(r.retune, channel);
        tx.configure(Configuration::AccessPoint {
            permission: r.access_point.configure.with_context(32),
            receive: ap_filter(),
        });
        let mut tx = tx.enable();
        tx.sample_tsf(Sampling::Station(r.station.tsf));
        let SetupCommand::Configure(Configuration::Station {
            permission,
            receive,
        }) = rx.next().await
        else {
            panic!("sta configure");
        };
        assert!(matches!(receive, StationReceive::OtherBssManagement));
        assert_eq!(permission.into_parts().1, 31);
        let SetupCommand::Retune { channel: got, .. } = rx.next().await else {
            panic!("retune");
        };
        assert_eq!(got, channel);
        let SetupCommand::Configure(Configuration::AccessPoint { permission, .. }) =
            rx.next().await
        else {
            panic!("ap configure");
        };
        assert_eq!(permission.into_parts().1, 32);
        let SetupCommand::Enable(enable) = rx.next().await else {
            panic!("enable last");
        };
        let mut rx = rx.enabled(enable);
        assert!(matches!(
            rx.next().await,
            PhaseCommand::SampleTsf(Sampling::Station(_))
        ));
        let tx = tx.disable();
        let PhaseCommand::Disable(disable) = rx.next().await else {
            panic!("disable");
        };
        let (_, reply) = rx.disabled(disable);
        let _tx = tx.finished(reply);
    }));
}

#[test]
fn disabling_returns_only_its_owned_places_and_many_cycles_wrap_safely() {
    ready(branded::<CommandPools<0, 0>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store = Storage::<0, 9>::new(r.bridge);
        let (mut tx, mut rx) = store.split(r.issuer, r.enable, r.disable);
        for _ in 0..128 {
            let phase = tx.enable();
            let SetupCommand::Enable(enable) = rx.next().await else {
                panic!("enable");
            };
            let mut phase_rx = rx.enabled(enable);
            let waiting = phase.disable();
            let PhaseCommand::Disable(disable) = phase_rx.next().await else {
                panic!("disable");
            };
            let (next_rx, reply) = phase_rx.disabled(disable);
            tx = waiting.finished(reply);
            rx = next_rx;
        }
    }));
}

struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn cancelling_one_command_wait_does_not_steal_the_next_waiters_wake() {
    ready(branded::<CommandPools<0, 0>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store = Storage::<0, 9>::new(r.bridge);
        let (tx, mut rx) = store.split(r.issuer, r.enable, r.disable);
        let first = Arc::new(Wakes(AtomicUsize::new(0)));
        let second = Arc::new(Wakes(AtomicUsize::new(0)));
        {
            let mut cancelled = pin!(rx.next());
            assert!(
                cancelled
                    .as_mut()
                    .poll(&mut Context::from_waker(&Waker::from(first.clone())))
                    .is_pending()
            );
        }
        let mut surviving = pin!(rx.next());
        assert!(
            surviving
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(second.clone())))
                .is_pending()
        );
        let _phase = tx.enable();
        assert_eq!(second.0.load(Ordering::SeqCst), 1);
        assert!(
            surviving
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready()
        );
    }));
}

#[test]
fn cross_core_fifo_moves_non_static_configuration_context_unchanged() {
    let station_context = 123_u32;
    let access_point_context = 456_u32;
    ready(branded::<CommandPools<0, 0>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store: CommandStorage<'_, Caps, &u32, &u32, (), (), 0, 0, 4, 9> =
            CommandStorage::new(r.bridge);
        let (mut tx, mut rx) = store.split(r.issuer, r.enable, r.disable);
        std::thread::scope(|threads| {
            threads
                .spawn(|| {
                    tx.configure(Configuration::Station {
                        permission: r.station.configure.with_context(&station_context),
                        receive: StationReceive::Off,
                    });
                    tx.configure(Configuration::AccessPoint {
                        permission: r.access_point.configure.with_context(&access_point_context),
                        receive: ap_filter(),
                    });
                    let _phase = tx.enable();
                })
                .join()
                .unwrap();
            let SetupCommand::Configure(Configuration::Station { permission, .. }) =
                ready(rx.next())
            else {
                panic!("station");
            };
            assert!(core::ptr::eq(permission.into_parts().1, &station_context));
            let SetupCommand::Configure(Configuration::AccessPoint { permission, .. }) =
                ready(rx.next())
            else {
                panic!("access point");
            };
            assert!(core::ptr::eq(
                permission.into_parts().1,
                &access_point_context
            ));
        });
    }));
}

#[test]
fn configured_reply_and_gate_barrier_return_the_exact_cell_permissions() {
    ready(branded::<CommandPools<0, 0>, _>(async |mint| {
        let r = mint.into_pools();
        let mut store = Storage::<0, 9>::new(r.bridge);
        let (setup, mut rx) = store.split(r.issuer, r.enable, r.disable);
        let mut tx = setup.enable();
        let SetupCommand::Enable(enable) = rx.next().await else {
            panic!("enable")
        };
        let mut rx = rx.enabled(enable);
        let mut configure = r.station.configure;
        let mut gate = r.gate;
        for generation in 0..128 {
            tx.configure(Configuration::Station {
                permission: configure.with_context(generation),
                receive: StationReceive::Off,
            });
            tx.close_gate(gate);
            let PhaseCommand::Configure(configuration) = rx.next().await else {
                panic!("configure")
            };
            // Hold the real terminal while the other command's cell is freed.
            let terminal = configuration.configured::<payload::Types>();
            let PhaseCommand::CloseGate(closing) = rx.next().await else {
                panic!("close")
            };
            let closed = tx.closed(rx.gate_barrier(closing));
            gate = tx.open_gate(closed);
            let LowerMacTerminal::Configured(Configured::Station(permission)) = terminal else {
                panic!("configured terminal")
            };
            let (returned, context) = permission.into_parts();
            assert_eq!(context, generation);
            configure = returned;
            assert!(matches!(rx.next().await, PhaseCommand::OpenGate));
        }
        let waiting = tx.disable();
        let PhaseCommand::Disable(disable) = rx.next().await else {
            panic!("disable")
        };
        let (_, reply) = rx.disabled(disable);
        let _setup = waiting.finished(reply);
    }));
}

fn assert_bijection(indices: impl Iterator<Item = usize>, capacity: usize) {
    let mut indices: Vec<_> = indices.collect();
    indices.sort_unstable();
    assert_eq!(indices, (0..capacity).collect::<Vec<_>>());
}

fn command_index_bijection<const T: usize, const K: usize>() {
    ready(branded::<CommandPools<T, K>, _>(async |mint| {
        let r = mint.into_pools();
        let setup: [SetupCommand<'_, Caps, (), ()>; 4] = [
            SetupCommand::Configure(Configuration::Station {
                permission: r.station.configure,
                receive: StationReceive::Off,
            }),
            SetupCommand::Configure(Configuration::AccessPoint {
                permission: r.access_point.configure,
                receive: ap_filter(),
            }),
            SetupCommand::Retune {
                permission: r.retune,
                channel: Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap(),
            },
            SetupCommand::Enable(Enable { _place: r.enable }),
        ];
        assert_bijection(
            setup.iter().map(SetupPlaces::index),
            CommandPools::<T, K>::SETUP_STORAGE,
        );
    }));
    ready(branded::<CommandPools<T, K>, _>(async |mint| {
        let r = mint.into_pools();
        let mut phase: Vec<PhaseCommand<'_, Caps, (), (), &str, payload::Mpdu>> = std::vec![
            PhaseCommand::Configure(Configuration::Station {
                permission: r.station.configure,
                receive: StationReceive::Off,
            }),
            PhaseCommand::Configure(Configuration::AccessPoint {
                permission: r.access_point.configure,
                receive: ap_filter(),
            }),
            PhaseCommand::SampleTsf(Sampling::Station(r.station.tsf)),
            PhaseCommand::SampleTsf(Sampling::AccessPoint(r.access_point.tsf)),
            PhaseCommand::CloseGate(ClosingGate { gate: r.gate }),
            PhaseCommand::OpenGate,
            PhaseCommand::Disable(Disable { _place: r.disable }),
        ];
        for (ticket, core_ticket) in r.tickets.into_iter().zip(r.core.tickets) {
            let ordinal = ticket.index();
            phase.push(PhaseCommand::Submit(Submission {
                operation: payload::admit("index permission", ticket, ordinal),
                admission: TxAdmission::Open(core_ticket),
            }));
        }
        for permission in r.installs {
            phase.push(PhaseCommand::InstallPairwise {
                permission,
                key: Ccmp128Key::new(&zeroize::Zeroizing::new([1; 16])),
            });
        }
        for permission in r.removes {
            phase.push(PhaseCommand::RemovePairwise(permission));
        }
        for permission in r.groups {
            phase.push(PhaseCommand::MutateGroup {
                permission,
                key: Ccmp128Key::new(&zeroize::Zeroizing::new([2; 16])),
            });
        }
        // These are real owners from Mint, including the first and last
        // index of every range. The expected image is the entire allocation,
        // independently of the order or arithmetic used by PhasePlaces.
        assert_bijection(
            phase.iter().map(PhasePlaces::<T, K>::index),
            CommandPools::<T, K>::PHASE_STORAGE,
        );
    }));
}

#[test]
fn every_command_variant_maps_bijectively_at_pool_boundaries() {
    command_index_bijection::<0, 0>();
    command_index_bijection::<1, 1>();
    command_index_bijection::<2, 2>();
    command_index_bijection::<3, 7>();
    command_index_bijection::<7, 3>();
    // Mint's full u8 index domain, and each empty neighboring range.
    command_index_bijection::<0, 256>();
    command_index_bijection::<256, 0>();
    command_index_bijection::<256, 256>();
}

#[test]
fn quarantine_erases_key_material_and_keeps_the_permission() {
    use oer_memory::Quarantine;
    ready(branded::<CommandPools<2, 1>, _>(async |mint| {
        let r = mint.into_pools();
        let [install] = r.installs;
        let mut command: PhaseCommand<'_, Caps, (), (), (), payload::Mpdu> =
            PhaseCommand::InstallPairwise {
                permission: install,
                key: Ccmp128Key::new(&zeroize::Zeroizing::new([7; 16])),
            };
        command.quarantine();
        let PhaseCommand::InstallPairwise { key, .. } = command else {
            panic!("quarantine keeps the command in place");
        };
        assert_eq!(key.as_bytes(), &[0; 16]);
    }));
}
