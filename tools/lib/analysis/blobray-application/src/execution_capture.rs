//! Phase-owned physical transfer capture; inspecting arguments never dispatches code/models.
use super::*;
impl Session<'_> {
    pub(super) fn capture_call(&mut self, input: &CallInput, c: &mut dyn RunControl) -> Result<()> {
        let Some((profile, _)) = &self.capture else {
            return Ok(());
        };
        if input.tail && !profile.include_tail {
            return Ok(());
        }
        c.checkpoint(profile.overrides.len().max(1).ilog2() as u64 + 1)?;
        let words = profile
            .overrides
            .binary_search_by_key(&input.target, |o| o.target)
            .map(|i| profile.overrides[i].words)
            .unwrap_or(profile.argument_words);
        if usize::from(words) + 1 > self.max_events - self.events.len() {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "physical call capture exceeds event capacity",
            ));
        }
        let target_kind = if self.calls.find(input.target, c)?.is_some() {
            ObservedCallTarget::CallModel
        } else {
            c.checkpoint(self.regions.len() as u64 + 1)?;
            if self.region_index(input.target, 2).is_some_and(|(i, o)| {
                let r = &self.regions[i];
                r.kind == RegionKind::Image && r.flags & 1 != 0 && !r.known[o..o + 2].contains(&0)
            }) {
                ObservedCallTarget::CapturedCode
            } else {
                ObservedCallTarget::Unavailable
            }
        };
        self.event(
            ExecutionEvent::CallTransfer {
                site: input.site,
                target: input.target,
                tail: input.tail,
                indirect: input.indirect,
                stack: input.stack,
                target_kind,
                words,
            },
            c,
        )?;
        for word in 0..words {
            c.checkpoint(self.regions.len() as u64 + 1)?;
            let value = if word < 8 {
                input.arguments[usize::from(word)]
                    .map(|value| ObservedWord::Known { value })
                    .unwrap_or(ObservedWord::Unknown)
            } else {
                self.stack_word(input.stack, word)
            };
            self.event(ExecutionEvent::TransferArgument { word, value }, c)?;
        }
        Ok(())
    }
    fn stack_word(&self, stack: Option<u32>, word: u16) -> ObservedWord {
        let unavailable = |reason| ObservedWord::Unavailable { reason };
        let Some(stack) = stack else {
            return unavailable(WordAccess::UnknownStack);
        };
        if !stack.is_multiple_of(16) {
            return unavailable(WordAccess::MisalignedStack);
        }
        let Some((i, o)) = stack
            .checked_add(u32::from(word - 8) * 4)
            .and_then(|address| self.region_index(address, 4))
        else {
            return unavailable(WordAccess::OutsidePrivateStack);
        };
        let r = &self.regions[i];
        if r.kind != RegionKind::Stack || r.flags & 4 == 0 {
            return unavailable(WordAccess::OutsidePrivateStack);
        }
        if r.known[o..o + 4].contains(&0) {
            ObservedWord::Unknown
        } else {
            ObservedWord::Known {
                value: u32::from_le_bytes(r.bytes[o..o + 4].try_into().unwrap()),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_refuses_partial_capacity_releases_owner_and_retains_unknown_stack() {
        let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
        let mut c = || Ok(());
        let mut s = Session::new(&memory, 10, Default::default(), &mut c).unwrap();
        let baseline = memory.used();
        let input = CallInput {
            site: 0x1000,
            target: 0x2000,
            tail: false,
            indirect: true,
            stack: None,
            arguments: [Some(0); 8],
        };
        for _ in 0..3 {
            let p = CallCapture {
                include_tail: false,
                argument_words: 9,
                overrides: vec![CallWordCount {
                    target: 0x2000,
                    words: 9,
                }],
            };
            let cap = memory.reserve(p.payload_bytes(), c.position()).unwrap();
            s.capture = Some((p, cap));
            s.capture_call(&input, &mut c).unwrap();
            assert_eq!(s.events.len(), 10);
            assert!(matches!(
                s.events[9],
                ExecutionEvent::TransferArgument {
                    word: 8,
                    value: ObservedWord::Unavailable {
                        reason: WordAccess::UnknownStack
                    }
                }
            ));
            assert_eq!(
                s.capture_call(&input, &mut c).unwrap_err().code,
                ErrorCode::ResourceLimited
            );
            assert_eq!(s.events.len(), 10);
            s.events.clear();
            s.finish_phase();
            // The session keeps its admitted event buffer for later phases.
            let retained = 10 * std::mem::size_of::<ExecutionEvent>() as u64;
            assert_eq!(memory.used(), baseline + retained);
        }
        drop(s);
        assert_eq!(memory.used(), 0);
    }
}
