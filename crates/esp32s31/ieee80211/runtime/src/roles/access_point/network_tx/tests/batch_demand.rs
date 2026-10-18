//! Per-destination batch demand over FIFO and destination-queue sources.

use crate::datapath::{
    PinnedTxPool, PinnedTxResources, SelectedBurstMaterializer, TxBatchDemand,
    owned::DatapathTxConsumer,
};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};

use std::boxed::Box;

use super::{
    super::{AccessPointNetworkTx, AccessPointTxStorage},
    fifo::Fifo,
    support::{allocator, packet, with_authorized_ap},
};

/// Peer `[4; 6]` holds a Block Ack agreement; `[6; 6]` and group traffic
/// receive single MPDUs.
const AGGREGATING: u8 = 4;
const SINGLE: u8 = 6;
const GROUP: u8 = 255;
const WINDOW: usize = 32;

fn target(destination: [u8; 6]) -> usize {
    if destination == [AGGREGATING; 6] {
        WINDOW
    } else {
        1
    }
}

type Source = DatapathTxConsumer<'static, 'static, NoopRawMutex, 64, 16, 8, 1>;

/// Production owned source plus a publisher of `(destination, sequence)`.
fn source<const SOURCE: usize>() -> (Source, impl FnMut(u8, u8)) {
    let pool = allocator::<SOURCE>();
    let resources = Box::leak(Box::new(
        OwnedEndpointResources::<NoopRawMutex, 1, SOURCE>::new(),
    ));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
    let radio = Box::leak(Box::new(radio));
    radio.link_controller().set_link_up(true);
    let dma = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let dma_resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let source = DatapathTxConsumer::new(radio, dma_resources.split(dma).for_interface(interface));
    let publish = move |destination, sequence| {
        device
            .transmit(packet(pool, destination, sequence))
            .unwrap();
    };
    (source, publish)
}

macro_rules! access_point {
    () => {
        AccessPointNetworkTx::new(Box::leak(Box::new(AccessPointTxStorage::new())), None)
    };
}

/// Retain every FIFO owner as the AP does while regrouping a start.
fn retain_all(
    ap: &mut AccessPointNetworkTx<
        '_,
        <Source as SelectedBurstMaterializer>::PhysicalFrame,
        <Source as SelectedBurstMaterializer>::SoftwareFrame,
    >,
    engine: &mut super::super::ApEngine<'_>,
    fifo: &Fifo<'_, Source>,
) {
    while let Some(frame) = fifo.try_take() {
        ap.retain_active_frame(engine, frame).unwrap();
    }
}

#[test]
fn a_single_mpdu_peer_completes_the_batch_behind_an_aggregating_backlog() {
    let (source, mut publish) = source::<8>();
    let source = &source;
    let mut ap = access_point!();
    with_authorized_ap(|engine| {
        let fifo = Fifo::<_>(source);
        publish(AGGREGATING, 0);
        publish(AGGREGATING, 1);
        assert_eq!(
            ap.batch_demand(target, &fifo),
            TxBatchDemand::single(2),
            "an unclassified owner has no known destination to wait for"
        );
        retain_all(&mut ap, engine, &fifo);
        assert_eq!(ap.active_frames.len(), 2);
        assert_eq!(
            ap.batch_demand(target, &fifo),
            TxBatchDemand {
                target: WINDOW,
                ready: 2
            },
            "an aggregating destination alone collects toward its window"
        );

        publish(SINGLE, 2);
        retain_all(&mut ap, engine, &fifo);
        assert_eq!(
            ap.batch_demand(target, &fifo),
            TxBatchDemand {
                target: 1,
                ready: 1
            },
            "a peer without a Block Ack agreement is never delayed"
        );
        assert_eq!(ap.active_frames.len(), 3);
    });
}

#[test]
fn group_traffic_completes_the_batch() {
    let (source, mut publish) = source::<8>();
    let source = &source;
    let mut ap = access_point!();
    with_authorized_ap(|engine| {
        let fifo = Fifo::<_>(source);
        publish(AGGREGATING, 0);
        publish(GROUP, 1);
        retain_all(&mut ap, engine, &fifo);
        assert!(ap.batch_demand(target, &fifo).complete());
    });
}

#[test]
fn destination_queues_are_inspected_in_place() {
    let (source, mut publish) = source::<8>();
    let source = &source;
    let ap = access_point!();
    with_authorized_ap(|_| {
        publish(AGGREGATING, 0);
        publish(AGGREGATING, 1);
        assert_eq!(
            ap.batch_demand(target, source),
            TxBatchDemand {
                target: WINDOW,
                ready: 2
            }
        );
        publish(SINGLE, 2);
        assert_eq!(
            ap.batch_demand(target, source),
            TxBatchDemand {
                target: 1,
                ready: 1
            }
        );
    });
}
