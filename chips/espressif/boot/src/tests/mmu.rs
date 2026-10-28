use std::vec::Vec;

use crate::{
    Access, BootFailure, MmuFailure,
    mmu::{MmuEntry, MmuRegisters, MmuTarget, MmuWindow, map, physical_address},
};

const WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x5000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: 1024,
    physical_pages: 1024,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Read(u16),
    Write(u16, MmuEntry),
}

/// An MMU table that logs every selector use; `corrupt` drops the next
/// write.
struct Table {
    entries: Vec<Option<MmuEntry>>,
    log: Vec<Op>,
    drop_writes: bool,
}

impl Table {
    fn new() -> Self {
        Self {
            entries: std::vec![None; 1024],
            log: Vec::new(),
            drop_writes: false,
        }
    }

    fn writes(&self) -> usize {
        self.log
            .iter()
            .filter(|op| matches!(op, Op::Write(..)))
            .count()
    }
}

impl MmuRegisters for Table {
    fn read_entry(&mut self, _: Access<'_>, index: u16) -> Option<MmuEntry> {
        self.log.push(Op::Read(index));
        self.entries.get(usize::from(index)).copied().flatten()
    }
    fn write_entry(&mut self, _: Access<'_>, index: u16, entry: MmuEntry) {
        self.log.push(Op::Write(index, entry));
        if !self.drop_writes {
            self.entries[usize::from(index)] = Some(entry);
        }
    }
}

const fn psram(page: u32) -> MmuEntry {
    MmuEntry {
        target: MmuTarget::Psram,
        page,
    }
}

#[test]
fn a_valid_entry_gives_its_physical_address() {
    let mut table = Table::new();
    table.entries[3] = Some(psram(7));
    assert_eq!(
        physical_address(&mut table, WINDOW, MmuTarget::Psram, 0x5003_0124),
        Ok(7 * 0x1_0000 + 0x124)
    );
}

#[test]
fn an_invalid_entry_is_never_page_zero() {
    let mut table = Table::new();
    assert_eq!(
        physical_address(&mut table, WINDOW, MmuTarget::Psram, 0x5000_0000),
        Err(BootFailure::Mmu(MmuFailure::Unmapped))
    );
}

#[test]
fn an_entry_of_the_other_target_or_outside_the_window_is_refused() {
    let mut table = Table::new();
    table.entries[0] = Some(MmuEntry {
        target: MmuTarget::Flash,
        page: 0,
    });
    assert_eq!(
        physical_address(&mut table, WINDOW, MmuTarget::Psram, 0x5000_0000),
        Err(BootFailure::Mmu(MmuFailure::WrongTarget))
    );
    assert_eq!(
        physical_address(&mut table, WINDOW, MmuTarget::Psram, 0x4fff_ffff),
        Err(BootFailure::Mmu(MmuFailure::OutsideWindow))
    );
    assert_eq!(
        physical_address(&mut table, WINDOW, MmuTarget::Psram, 0x5400_0000),
        Err(BootFailure::Mmu(MmuFailure::OutsideWindow))
    );
}

#[test]
fn a_range_maps_entry_by_entry_each_read_back() {
    let mut table = Table::new();
    let mapped = map(
        &mut table,
        WINDOW,
        MmuTarget::Psram,
        0x5000_0000,
        0,
        0x3_0000,
    )
    .expect("mapped");
    assert_eq!(mapped.len, 0x3_0000);
    // Every entry is checked free first, then written and read back.
    assert_eq!(
        &table.log[3..],
        &[
            Op::Write(0, psram(0)),
            Op::Read(0),
            Op::Write(1, psram(1)),
            Op::Read(1),
            Op::Write(2, psram(2)),
            Op::Read(2),
        ]
    );
}

#[test]
fn an_entry_mapping_another_page_refuses_the_range_unwritten() {
    let mut table = Table::new();
    table.entries[2] = Some(psram(9));
    assert_eq!(
        map(
            &mut table,
            WINDOW,
            MmuTarget::Psram,
            0x5000_0000,
            0,
            0x3_0000
        ),
        Err(BootFailure::Mmu(MmuFailure::Occupied))
    );
    assert_eq!(table.writes(), 0);
}

#[test]
fn an_entry_already_mapping_the_same_page_is_kept() {
    let mut table = Table::new();
    table.entries[1] = Some(psram(1));
    let _mapped = map(
        &mut table,
        WINDOW,
        MmuTarget::Psram,
        0x5000_0000,
        0,
        0x3_0000,
    )
    .expect("mapped");
    assert_eq!(table.writes(), 3);
}

#[test]
fn misaligned_out_of_window_or_too_far_ranges_are_refused_unwritten() {
    let mut table = Table::new();
    for (virtual_start, physical, len, failure) in [
        (0x5000_0000, 0, 0x8000, MmuFailure::Misaligned),
        (0x5000_0000, 0x8000, 0x1_0000, MmuFailure::Misaligned),
        (0x5000_8000, 0, 0x1_0000, MmuFailure::Misaligned),
        (0x5000_0000, 0, 0, MmuFailure::Misaligned),
        (0x53ff_0000, 0, 0x2_0000, MmuFailure::OutsideWindow),
        (
            0x5000_0000,
            1023 * 0x1_0000,
            0x2_0000,
            MmuFailure::PhysicalRange,
        ),
    ] {
        assert_eq!(
            map(
                &mut table,
                WINDOW,
                MmuTarget::Psram,
                virtual_start,
                physical,
                len
            ),
            Err(BootFailure::Mmu(failure)),
            "{virtual_start:#x} {physical:#x} {len:#x}"
        );
    }
    assert_eq!(table.writes(), 0);
}

#[test]
fn an_entry_that_does_not_read_back_stops_the_mapping() {
    let mut table = Table::new();
    table.drop_writes = true;
    assert_eq!(
        map(
            &mut table,
            WINDOW,
            MmuTarget::Psram,
            0x5000_0000,
            0,
            0x2_0000
        ),
        Err(BootFailure::Mmu(MmuFailure::Readback))
    );
    assert_eq!(table.writes(), 1);
}

#[test]
fn an_overflowing_selector_is_refused_before_register_access() {
    let window = MmuWindow {
        first_entry: u16::MAX,
        entries: 2,
        ..WINDOW
    };
    assert_eq!(window.locate(window.virtual_base), Some((u16::MAX, 0)));
    let second_page = window.virtual_base + window.page_bytes;
    assert_eq!(window.locate(second_page), None);

    let mut table = Table::new();
    assert_eq!(
        physical_address(&mut table, window, MmuTarget::Psram, second_page),
        Err(BootFailure::Mmu(MmuFailure::OutsideWindow))
    );
    assert_eq!(
        map(
            &mut table,
            window,
            MmuTarget::Psram,
            window.virtual_base,
            0,
            2 * window.page_bytes,
        ),
        Err(BootFailure::Mmu(MmuFailure::OutsideWindow))
    );
    assert!(table.log.is_empty(), "no selector was read or written");
}

#[test]
fn the_last_representable_selector_can_be_mapped() {
    let window = MmuWindow {
        first_entry: u16::MAX,
        entries: 1,
        ..WINDOW
    };
    let mut table = Table::new();
    table.entries.resize(usize::from(u16::MAX) + 1, None);
    let mapped = map(
        &mut table,
        window,
        MmuTarget::Psram,
        window.virtual_base,
        0,
        window.page_bytes,
    )
    .expect("last selector fits");
    assert_eq!(mapped.len, window.page_bytes);
    assert_eq!(
        table.log,
        [
            Op::Read(u16::MAX),
            Op::Write(u16::MAX, psram(0)),
            Op::Read(u16::MAX),
        ]
    );
}

#[test]
fn invalid_page_sizes_and_address_overflow_are_refused_unwritten() {
    let mut table = Table::new();
    for page_bytes in [0, 3] {
        let window = MmuWindow {
            page_bytes,
            ..WINDOW
        };
        assert_eq!(window.locate(window.virtual_base), None);
        assert_eq!(
            map(
                &mut table,
                window,
                MmuTarget::Psram,
                window.virtual_base,
                0,
                12
            ),
            Err(BootFailure::Mmu(MmuFailure::Misaligned))
        );
    }
    let window = MmuWindow {
        virtual_base: 0xffff_0000,
        ..WINDOW
    };
    assert_eq!(
        map(
            &mut table,
            window,
            MmuTarget::Psram,
            window.virtual_base,
            0,
            0x2_0000
        ),
        Err(BootFailure::Mmu(MmuFailure::OutsideWindow))
    );
    assert!(table.log.is_empty());
}
