//! The low-level interface over host memory: each accessor reaches the
//! register its public LL counterpart names, and reads back what it wrote.

use super::*;
use crate::ieee802154::mac::host::{HostMemory, changed, silent};
use oer_espressif_ieee802154_engine::ll::COEX_DISABLED_PTI;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154Event, Ieee802154MultipanEnableState, Ieee802154MultipanIndex,
};

#[test]
fn each_command_names_its_one_image() {
    let commands = [
        (Ieee802154MacCommand::Transmit, Opcode::TxStart),
        (Ieee802154MacCommand::Receive, Opcode::RxStart),
        (
            Ieee802154MacCommand::ClearChannelThenTransmit,
            Opcode::CcaTxStart,
        ),
        (Ieee802154MacCommand::EnergyDetection, Opcode::EdStart),
        (Ieee802154MacCommand::Stop, Opcode::Stop),
    ];
    for (command, image) in commands {
        assert_eq!(opcode(command), image);
    }
    assert_eq!(timer_start(Ieee802154Timer::Timer0), Opcode::Timer0Start);
    assert_eq!(timer_stop(Ieee802154Timer::Timer0), Opcode::Timer0Stop);
    assert_eq!(timer_start(Ieee802154Timer::Timer1), Opcode::Timer1Start);
    assert_eq!(timer_stop(Ieee802154Timer::Timer1), Opcode::Timer1Stop);
}

#[test]
fn a_timer_threshold_is_written_whole() {
    let before = HostMemory::filled(0);
    let mut memory = HostMemory::filled(0);
    {
        let (mut task, _setup) = memory.owners();
        task.set_timer1_threshold(0x8765_4321);
    }
    assert_eq!(changed(&before.mac, &memory.mac), [0x8765_4321]);
}

#[test]
fn the_pib_accessors_read_back_what_they_write() {
    let mut memory = HostMemory::filled(0);
    let (task, setup) = memory.owners();
    let (startup, interrupts) = setup.arm(task);
    let mut owners = Ieee802154MacOwners::new(startup, interrupts, silent());

    owners.set_tx_auto_ack(true);
    owners.set_rx_auto_ack(true);
    owners.set_tx_enhanced_ack(true);
    owners.set_pending_mode(true);
    assert!(owners.tx_auto_ack() && owners.rx_auto_ack());
    assert!(owners.tx_enhanced_ack() && owners.pending_mode());
    owners.set_rx_auto_ack(false);
    assert!(owners.tx_auto_ack() && !owners.rx_auto_ack());

    let channel = Ieee802154Channel::new(26).expect("standard channel");
    owners.set_channel(channel);
    assert_eq!(owners.frequency_code(), channel.frequency_code());
    owners.set_ack_timeout(0x1234);
    assert_eq!(owners.ack_timeout(), 0x1234);

    let context = Ieee802154MultipanIndex::CONTEXT1;
    owners.set_multipan_panid(context, 0xface);
    assert_eq!(owners.multipan_panid(context), 0xface);
    assert_eq!(
        owners.multipan_enable(),
        Ieee802154MultipanEnableState::new(false, true, false, false)
    );
    owners.set_multipan_enable(Ieee802154MultipanEnableState::ALL);
    assert_eq!(owners.multipan_enable(), Ieee802154MultipanEnableState::ALL);

    let (mut task, interrupts, _silent) = owners.into_parts();
    let _setup = interrupts.deactivate(&mut task);
}

#[test]
fn disabling_coexistence_writes_the_disabled_priority_to_both_fields() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, setup) = memory.owners();
    let (startup, interrupts) = setup.arm(task);
    let mut owners = Ieee802154MacOwners::new(startup, interrupts, silent());
    owners.disable_coex();
    let pti = owners.task.mac_part().read_coex_pti();
    assert_eq!(pti.txrx_pti().get(), u32::from(COEX_DISABLED_PTI));
    assert_eq!(pti.ack_pti().get(), u32::from(COEX_DISABLED_PTI));
    assert!(
        pti.close_rf_sel(),
        "the register's other fields keep their value"
    );

    let shared = CoexPti::new(9).expect("four-bit priority");
    owners.set_txrx_pti(shared);
    assert_eq!(owners.task.mac_part().read_coex_pti().txrx_pti().get(), 9);

    let (mut task, interrupts, _silent) = owners.into_parts();
    let _setup = interrupts.deactivate(&mut task);
}

#[test]
fn an_event_enable_reaches_only_its_event() {
    let mut memory = HostMemory::filled(0);
    let (task, setup) = memory.owners();
    let (startup, interrupts) = setup.arm(task);
    let mut owners = Ieee802154MacOwners::new(startup, interrupts, silent());
    // Arming left every event masked; the engine enables them.
    owners.enable_all_events();
    let events = owners.task.mac_part().read_event_enable();
    assert!(events.tx_done() && events.events_7() && events.events_13());

    owners.disable_event(Ieee802154Event::TxDone);
    owners.disable_event(Ieee802154Event::Timer0Overflow);
    owners.enable_event(Ieee802154Event::Timer0Overflow);
    let events = owners.task.mac_part().read_event_enable();
    assert!(!events.tx_done() && events.rx_done());
    assert!(events.timer0_overflow() && events.clock_count_match());

    let (mut task, interrupts, _silent) = owners.into_parts();
    let _setup = interrupts.deactivate(&mut task);
}
