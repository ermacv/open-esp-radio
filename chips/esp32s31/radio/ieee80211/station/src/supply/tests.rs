extern crate std;

use super::*;
use oer_chips_esp32s31_hal::system::memory::dma_static::Placed;
use oer_portable_memory::PinnedDmaTxPool;
use std::boxed::Box;

fn pool() -> &'static TxPool<TX> {
    let storage = Box::leak(Box::new(PinnedDmaTxPool::new()));
    let Ok(pool) = TxPool::pin(Placed::host(storage)) else {
        panic!("a host pool pins");
    };
    Box::leak(Box::new(pool))
}

#[test]
fn the_network_holds_every_buffer_the_port_does_not() {
    let pool = pool();
    let tx = NetworkTx::claim(pool, PORT_BUFFERS).unwrap();
    let (_, slots) = tx.into_parts();
    assert_eq!(slots.count(), NETWORK_BUFFERS);
    // The port's buffers stay free for the port.
    assert!(pool.claim_network(0).is_ok());
}

#[test]
fn a_frame_goes_to_the_station_where_the_data_machine_reads_it_and_comes_back_writable() {
    let pool = pool();
    let (publisher, mut slots) = NetworkTx::claim(pool, PORT_BUFFERS).unwrap().into_parts();
    let mut slot = slots.next().unwrap();
    // The stack wrote the frame 6 octets further in.
    slot.bytes_mut()[6..26].copy_from_slice(&[0x5a; 20]);
    let Ok(memory) = publisher.publish(slot, 6, 20) else {
        panic!("a frame inside its buffer is published");
    };
    assert_eq!(memory.ethernet().len(), ETHERNET_AT + 20);
    assert_eq!(memory.ethernet()[ETHERNET_AT..], [0x5a; 20]);
    let mut slot = publisher.slot(memory);
    assert_eq!(slot.bytes_mut()[..20], [0x5a; 20]);
}

#[test]
fn a_frame_past_its_buffer_keeps_the_buffer() {
    let pool = pool();
    let (publisher, mut slots) = NetworkTx::claim(pool, PORT_BUFFERS).unwrap().into_parts();
    let mut slot = slots.next().unwrap();
    let room = slot.bytes_mut().len();
    let Err(TxPublishRefused::Range(mut slot)) = publisher.publish(slot, 1, room) else {
        panic!("a frame past the buffer is refused");
    };
    assert_eq!(slot.bytes_mut().len(), room);
}

#[test]
fn each_network_buffer_has_its_own_place_below_the_network_s_count() {
    let pool = pool();
    let (_, slots) = NetworkTx::claim(pool, PORT_BUFFERS).unwrap().into_parts();
    let mut places: std::vec::Vec<u8> = slots.map(|slot| slot.place()).collect();
    places.sort_unstable();
    let every: std::vec::Vec<u8> = (0..u8::try_from(NETWORK_BUFFERS).unwrap()).collect();
    assert_eq!(places, every);
}

#[test]
fn an_access_point_s_network_holds_every_buffer_past_its_bank() {
    let pool = pool();
    let bank = 8;
    let (_, slots) = NetworkTx::claim(pool, bank).unwrap().into_parts();
    assert_eq!(slots.count(), TX_BUFFERS - usize::from(bank));
    // The bank's places stay free for the port.
    for place in 0..bank {
        assert!(pool.claim_network(place).is_ok());
    }
}
