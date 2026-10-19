//! Guest memory capture uses instruction identity, never progress metadata or inspection reads.
use super::*;
impl Session<'_> {
    pub(super) fn admit_memory_event(&self, selected: bool, c: &mut dyn RunControl) -> Result<()> {
        if !selected {
            return Ok(());
        }
        c.checkpoint(1)?;
        if self.pc.is_none() {
            return Err(Error::new(
                ErrorCode::Integrity,
                "memory observation lacks instruction owner",
            ));
        }
        if self.events.len() == self.max_events {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "normal-memory event capacity exhausted",
            ));
        }
        Ok(())
    }
    pub(super) fn trace_memory(
        &mut self,
        transaction: MemoryTransaction,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if !self.timeline.selects(&transaction) {
            return Ok(());
        }
        self.admit_memory_event(true, c)?;
        self.event(
            ExecutionEvent::Memory {
                site: self.pc.unwrap(),
                transaction,
            },
            c,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trace_admission_precedes_atomic_callback_and_unreadable_memory_stays_explicit() {
        let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
        let mut c = || Ok(());
        let mut s = Session::new(&memory, 1, Default::default(), &mut c).unwrap();
        s.region(
            Mapping {
                address: 0x3000,
                length: 4,
                flags: 6,
                kind: RegionKind::Ram(RegionLifetime::Session),
            },
            Some(0),
            &[],
            &mut c,
        )
        .unwrap();
        s.instruction(0x1000);
        s.timeline = TimelineCapture {
            reads: true,
            atomics: true,
            ..Default::default()
        };
        assert_eq!(
            s.read(0x3000, 4, MemoryAccess::Read, &mut c).unwrap(),
            Some(0)
        );
        let mut called = false;
        assert_eq!(
            s.modify_word(
                0x3000,
                ExecutionOrdering {
                    acquire: false,
                    release: false
                },
                &mut |_| {
                    called = true;
                    9
                },
                &mut c
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimited
        );
        assert!(!called);
        assert_eq!(&s.regions[0].bytes[..], &[0; 4]);
        s.events.clear();
        s.regions[0].flags = 2;
        assert_eq!(s.read(0x3000, 4, MemoryAccess::Read, &mut c).unwrap(), None);
        assert!(matches!(
            s.events[0],
            ExecutionEvent::Memory {
                site: 0x1000,
                transaction: MemoryTransaction::Read {
                    value: MemoryReadValue::Unavailable,
                    ..
                }
            }
        ));
        s.finish_phase();
        assert!(s.pc.is_none());
        assert!(!s.timeline.any());
    }

    #[test]
    fn written_ranges_coalesce_persistent_stores_only_and_stay_bounded() {
        let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
        let mut c = || Ok(());
        let mut s = Session::new(&memory, 1, Default::default(), &mut c).unwrap();
        let persistent = MAX_WRITTEN_RANGES * 2 + 2;
        for (address, length, lifetime) in [
            (0x3000, persistent, RegionLifetime::Session),
            (0x9000, 4, RegionLifetime::Phase),
        ] {
            s.region(
                Mapping {
                    address,
                    length,
                    flags: 6,
                    kind: RegionKind::Ram(lifetime),
                },
                Some(0),
                &[],
                &mut c,
            )
            .unwrap();
        }
        s.instruction(0x1000);
        assert!(s.write(0x3000, 4, 1, &mut c).unwrap());
        assert!(s.written.is_empty());
        s.timeline.written = true;
        for (address, width) in [(0x3004, 4), (0x3000, 1), (0x3001, 2), (0x9000, 4)] {
            assert!(s.write(address, width, 1, &mut c).unwrap());
        }
        let range = |address, length| WrittenRange { address, length };
        assert_eq!(s.written, [range(0x3000, 3), range(0x3004, 4)]);
        assert!(s.write(0x3003, 1, 1, &mut c).unwrap());
        assert_eq!(s.written, [range(0x3000, 8)]);
        s.written.clear();
        for i in 0..MAX_WRITTEN_RANGES as u32 {
            assert!(s.write(0x3000 + 2 * i, 1, 1, &mut c).unwrap());
        }
        assert_eq!(
            s.write(0x3000 + persistent as u32 - 1, 1, 1, &mut c)
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        // A store joining two ranges stays within capacity.
        assert!(s.write(0x3001, 1, 1, &mut c).unwrap());
        assert_eq!(s.written[0], range(0x3000, 3));
        assert_eq!(s.written.len(), MAX_WRITTEN_RANGES - 1);
    }
}
