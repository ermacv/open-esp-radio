//! Validate complete responses before mutating normal memory; no MMIO output fallback.
use super::*;
impl Session<'_> {
    pub(super) fn read_call_words(
        &self,
        input: &CallInput,
        words: u16,
        arguments: &mut [Option<u32>; MAX_EXECUTION_ARGUMENT_WORDS],
        c: &mut dyn RunControl,
    ) -> Result<Option<CallIssue>> {
        let Some(stack) = input.stack.filter(|s| s.is_multiple_of(16)) else {
            return Ok(Some(CallIssue::StackAlignment));
        };
        for word in 0..words {
            c.checkpoint(1)?;
            arguments[usize::from(word)] = if word < 8 {
                input.arguments[usize::from(word)]
            } else {
                let Some(address) = stack.checked_add(u32::from(word - 8) * 4) else {
                    return Ok(Some(CallIssue::StackArgument { word }));
                };
                let Some((i, o)) = self.region_index(address, 4) else {
                    return Ok(Some(CallIssue::StackArgument { word }));
                };
                let r = &self.regions[i];
                if r.kind != RegionKind::Stack || r.flags & 4 == 0 {
                    return Ok(Some(CallIssue::StackArgument { word }));
                }
                if r.known[o..o + 4].contains(&0) {
                    None
                } else {
                    Some(u32::from_le_bytes(r.bytes[o..o + 4].try_into().unwrap()))
                }
            };
        }
        Ok(None)
    }

    pub(super) fn output_location(
        &self,
        address: u32,
        width: u8,
        scope: CallOutputScope,
    ) -> Option<(usize, usize)> {
        if !address.is_multiple_of(u32::from(width)) {
            return None;
        }
        let (i, o) = self.region_index(address, width)?;
        let r = &self.regions[i];
        (r.flags & 2 != 0
            && (scope == CallOutputScope::NormalMemory || r.kind == RegionKind::Stack))
            .then_some((i, o))
    }
    pub(super) fn external_call(
        &mut self,
        input: &CallInput,
        c: &mut dyn RunControl,
    ) -> Result<CallDispatch> {
        let Some(slot) = self.calls.find(input.target, c)? else {
            return Ok(CallDispatch::Code);
        };
        macro_rules! gap {
            ($issue:expr) => {
                return Ok(self.calls.fail(slot, $issue))
            };
        }
        let binding = self.calls.declaration(slot).binding;
        if input.tail && !binding.allow_tail {
            gap!(CallIssue::TailNotAllowed);
        }
        let response = self.calls.consumed(slot);
        let words = self.calls.declaration(slot).argument_words;
        if self.calls.response(slot).is_none() {
            gap!(CallIssue::ExhaustedResponses);
        }
        let mut arguments = [None; MAX_EXECUTION_ARGUMENT_WORDS];
        if let Some(issue) = self.read_call_words(input, words, &mut arguments, c)? {
            gap!(issue);
        }
        let r = self.calls.response(slot).unwrap();
        let (return_words, allocation, delay, outputs) = (
            r.return_words,
            r.allocation,
            r.delay_micros,
            r.outputs.len(),
        );
        // Arguments/effects are separate fixed-size evidence records. Admit the whole successful response.
        let required = 2
            + usize::from(words)
            + outputs
            + usize::from(allocation.is_some())
            + usize::from(delay.is_some());
        if required > self.max_events - self.events.len() {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "modeled call exceeds event capacity",
            ));
        }
        self.event(
            ExecutionEvent::ModeledCall {
                site: input.site,
                target: input.target,
                tail: input.tail,
                response,
                boundary: binding.boundary,
            },
            c,
        )?;
        for word in 0..words {
            self.event(
                ExecutionEvent::CallArgument {
                    word,
                    value: arguments[usize::from(word)],
                },
                c,
            )?;
        }
        macro_rules! argument {
            ($word:expr) => {
                match arguments[usize::from($word)] {
                    Some(v) => v,
                    None => gap!(CallIssue::UnknownArgument { word: $word }),
                }
            };
        }
        // Bounded offsets are scratch control state, never persistent pointer graphs.
        let mut addresses = [0u32; MAX_CALL_OUTPUTS];
        for (index, address) in addresses.iter_mut().enumerate().take(outputs) {
            c.checkpoint(self.regions.len() as u64 + 1)?;
            let output = self.calls.response(slot).unwrap().outputs[index];
            let pointer = argument!(output.pointer_argument);
            let Some(a) = pointer.checked_add(output.byte_offset) else {
                gap!(CallIssue::OutputAddress {
                    word: output.pointer_argument
                });
            };
            if self
                .output_location(a, output.width, output.scope)
                .is_none()
            {
                gap!(CallIssue::OutputAccess {
                    address: a,
                    scope: output.scope
                });
            }
            *address = a;
        }
        let requested = if let Some(a) = allocation {
            let requested = argument!(a.size_argument);
            if requested > a.capacity {
                gap!(CallIssue::AllocationSize {
                    requested,
                    capacity: a.capacity
                });
            }
            let end = u64::from(a.address) + u64::from(a.capacity);
            c.checkpoint((self.regions.len() + MAX_CALL_MODELS) as u64)?;
            if self.devices.overlaps(a.address, u64::from(a.capacity), c)?
                || self.regions.iter().any(|r| {
                    u64::from(a.address) < u64::from(r.address) + r.bytes.len() as u64
                        && u64::from(r.address) < end
                })
                || self.calls.bindings().any(|b| {
                    u64::from(a.address) < u64::from(b.address) + 2 && u64::from(b.address) < end
                })
            {
                gap!(CallIssue::AllocationOverlap {
                    address: a.address,
                    capacity: a.capacity
                });
            }
            requested
        } else {
            0
        };
        let delay = match delay {
            None => None,
            Some(CallValue::Constant { value }) => Some(value),
            Some(CallValue::Argument { word }) => Some(argument!(word)),
        };
        // All semantic checks passed. Resource/cancellation failure below aborts publication.
        if let Some(a) = allocation {
            self.region(
                Mapping {
                    address: a.address,
                    length: a.capacity as usize,
                    flags: 6,
                    kind: RegionKind::Allocation {
                        lifetime: a.lifetime,
                        requested,
                    },
                },
                Some(0),
                &[],
                c,
            )?;
            self.event(
                ExecutionEvent::Allocation {
                    address: a.address,
                    requested,
                    capacity: a.capacity,
                    lifetime: a.lifetime,
                },
                c,
            )?;
        }
        for (index, address) in addresses.iter().copied().enumerate().take(outputs) {
            c.checkpoint(1)?;
            let output = self.calls.response(slot).unwrap().outputs[index];
            let (i, o) = self
                .output_location(address, output.width, output.scope)
                .ok_or_else(|| {
                    Error::new(ErrorCode::Integrity, "validated call output lost owner")
                })?;
            let r = &mut self.regions[i];
            let width = usize::from(output.width);
            r.bytes[o..o + width].copy_from_slice(&output.value.to_le_bytes()[..width]);
            r.known[o..o + width].fill(1);
            self.log(
                crate::execution_steps::StepEntry::Output {
                    address,
                    width: output.width,
                },
                c,
            )?;
            self.invalidate_reservation(address, output.width);
            self.event(
                ExecutionEvent::CallOutput {
                    address,
                    width: output.width,
                    value: output.value,
                    scope: output.scope,
                },
                c,
            )?;
        }
        if let Some(value) = delay {
            self.event(ExecutionEvent::DelayMicros { value }, c)?;
        }
        self.event(
            ExecutionEvent::CallReturn {
                words: return_words,
            },
            c,
        )?;
        self.calls.consume(slot);
        Ok(CallDispatch::Returned {
            words: return_words,
        })
    }
}
