//! Bounded snapshots of selected normal memory at the phase stop boundary.
use super::*;
impl Session<'_> {
    pub(crate) fn capture_final_memory(
        &mut self,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        input.validate_memory_selection()?;
        let count = input.memory_chunks();
        let capacity = self.memory.reserve(
            (count * std::mem::size_of::<FinalMemoryChunk>()) as u64,
            c.position(),
        )?;
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(count).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "final-memory snapshot allocation refused",
            )
        })?;
        for (selection, span) in input.observe_memory.iter().enumerate() {
            let mut offset = 0;
            while offset < span.length {
                let length = (span.length - offset).min(MEMORY_CHUNK_BYTES as u32) as u8;
                let address = span.address + offset;
                let mut chunk = FinalMemoryChunk {
                    selection: selection as u16,
                    offset,
                    length,
                    bytes: [0; MEMORY_CHUNK_BYTES],
                    available: 0,
                    known: 0,
                };
                c.checkpoint(self.regions.len() as u64 + 1)?;
                if let Some((region, start)) = self.region_index(address, length) {
                    let r = &self.regions[region];
                    if r.flags & 4 != 0 {
                        for i in 0..length as usize {
                            chunk.available |= 1 << i;
                            if r.known[start + i] != 0 {
                                chunk.known |= 1 << i;
                                chunk.bytes[i] = r.bytes[start + i];
                            }
                        }
                    }
                } else {
                    for i in 0..length as usize {
                        c.checkpoint(self.regions.len() as u64 + 1)?;
                        if let Some((region, start)) = self.region_index(address + i as u32, 1) {
                            let r = &self.regions[region];
                            if r.flags & 4 != 0 {
                                chunk.available |= 1 << i;
                                if r.known[start] != 0 {
                                    chunk.known |= 1 << i;
                                    chunk.bytes[i] = r.bytes[start];
                                }
                            }
                        }
                    }
                }
                chunks.push(chunk);
                offset += u32::from(length);
            }
        }
        self.final_memory = chunks;
        self.final_memory_capacity = Some(capacity);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_capacity_is_admitted_before_allocation_and_released_after_recycle() {
        let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
        let c = &mut || Ok(());
        let mut s = Session::new(&memory, 1, Default::default(), c).unwrap();
        let input = Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
            observe_memory: vec![MemorySelection {
                name: "unavailable".into(),
                address: 0x3000,
                length: 65536,
            }],
        };
        let base = memory.used();
        let bytes = (input.memory_chunks() * std::mem::size_of::<FinalMemoryChunk>()) as u64;
        let hold = memory
            .reserve(
                memory.observation().limit_bytes - base - bytes + 1,
                c.position(),
            )
            .unwrap();
        assert_eq!(
            s.capture_final_memory(&input, c).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
        assert!(s.final_memory.is_empty());
        drop(hold);
        assert_eq!(memory.used(), base);
        for _ in 0..3 {
            s.capture_final_memory(&input, c).unwrap();
            assert_eq!(memory.used(), base + bytes);
            let o = s
                .observation(
                    ExecutionStop::Returned {
                        low: Some(0),
                        high: None,
                    },
                    1,
                    true,
                    c,
                )
                .unwrap();
            assert_eq!(o.final_memory.len(), input.memory_chunks());
            assert!(
                o.final_memory
                    .iter()
                    .all(|c| c.available == 0 && c.known == 0)
            );
            s.recycle(o);
            assert_eq!(memory.used(), base);
        }
        drop(s);
        assert_eq!(memory.used(), 0);
    }
}
