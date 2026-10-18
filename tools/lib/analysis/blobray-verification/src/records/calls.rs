//! Structural validation of external-call lifetime evidence; no machine execution.
use super::*;
struct Live<'a> {
    declaration: &'a CallDeclaration,
    definition: ArtifactId,
    calls: u32,
    issue: Option<CallIssue>,
    seen: bool,
    closed: bool,
}
pub(super) struct Calls<'a> {
    live: Vec<Live<'a>>,
    pending: Option<Pending>,
}
impl<'a> Calls<'a> {
    pub fn new() -> Self {
        Self {
            live: Vec::new(),
            pending: None,
        }
    }
    pub fn begin(
        &mut self,
        declarations: &'a [CallDeclaration],
        reset: SessionReset,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if reset == SessionReset::Cold && !self.live.is_empty() {
            return Err(integrity("cold reset has unclosed call models"));
        }
        for i in &mut self.live {
            i.seen = false;
        }
        if blocked {
            return Ok(());
        }
        for d in declarations {
            c.checkpoint(self.live.len() as u64 + 1)?;
            if self.live.len() == MAX_CALL_MODELS
                || self.live.iter().any(|i| {
                    i.declaration.id == d.id || i.declaration.binding.address == d.binding.address
                })
            {
                return Err(integrity("duplicate or excessive live call models"));
            }
            self.live.try_reserve(1).map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "call model validation allocation refused",
                )
            })?;
            self.live.push(Live {
                declaration: d,
                definition: d.identity(c)?,
                calls: 0,
                issue: None,
                seen: false,
                closed: false,
            });
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        o: &CallObservation,
        close_chain: bool,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(self.live.len() as u64 + 1)?;
        let i = self
            .live
            .iter_mut()
            .find(|i| i.declaration.id == o.id)
            .ok_or_else(|| integrity("call observation has no live declaration"))?;
        if i.seen
            || o.definition != i.definition
            || o.lifetime != i.declaration.lifetime
            || o.closed != (close_chain || o.lifetime == RegionLifetime::Phase)
            || o.status != o.expected_status()
            || o.calls != i.calls
            || (i.issue.is_some() && o.issue != i.issue)
            || (blocked && (o.calls != i.calls || o.issue != i.issue))
            || o.remaining != i.declaration.remaining(o.calls)
            || (i.declaration.repetition == CallRepetition::Finite
                && o.calls as usize > i.declaration.responses.len())
        {
            return Err(integrity("call identity, counters or closure differs"));
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.address == i.declaration.binding.address)
        {
            if o.issue.is_none() {
                return Err(integrity("unfinished call trace has no model issue"));
            }
            self.pending = None;
        }
        i.seen = true;
        i.closed = o.closed;
        i.calls = o.calls;
        i.issue = o.issue;
        Ok(())
    }
    pub fn finish_side(&mut self) -> Result<()> {
        if self.pending.is_some() || self.live.iter().any(|i| !i.seen) {
            return Err(integrity("missing call participation evidence"));
        }
        self.live.retain(|i| !i.closed);
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.live.is_empty()
    }
}

struct Pending {
    slot: usize,
    address: u32,
    words: u16,
    arguments: [Option<u32>; MAX_EXECUTION_ARGUMENT_WORDS],
    outputs: usize,
    allocation: bool,
    delay: bool,
}
impl Calls<'_> {
    pub fn event(&mut self, event: &ExecutionEvent, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(self.live.len() as u64 + 1)?;
        if let ExecutionEvent::ModeledCall {
            target,
            tail,
            response,
            boundary,
            ..
        } = event
        {
            if self.pending.is_some() {
                return Err(integrity("nested model response trace"));
            }
            let slot = self
                .live
                .iter()
                .position(|i| i.declaration.binding.address == *target)
                .ok_or_else(|| integrity("call trace has no selected binding"))?;
            let i = &self.live[slot];
            if *response != i.calls
                || i.declaration.response(i.calls).is_none()
                || *boundary != i.declaration.binding.boundary
                || (*tail && !i.declaration.binding.allow_tail)
            {
                return Err(integrity("call trace differs from bound response"));
            }
            self.pending = Some(Pending {
                slot,
                address: *target,
                words: 0,
                arguments: [None; MAX_EXECUTION_ARGUMENT_WORDS],
                outputs: 0,
                allocation: false,
                delay: false,
            });
            return Ok(());
        }
        let Some(p) = &mut self.pending else {
            return if matches!(
                event,
                ExecutionEvent::CallArgument { .. }
                    | ExecutionEvent::CallReturn { .. }
                    | ExecutionEvent::CallOutput { .. }
                    | ExecutionEvent::Allocation { .. }
                    | ExecutionEvent::DelayMicros { .. }
            ) {
                Err(integrity("model effect has no call trace"))
            } else {
                Ok(())
            };
        };
        let i = &self.live[p.slot];
        let declaration = i.declaration;
        let r = declaration
            .response(i.calls)
            .ok_or_else(|| integrity("call trace has no bound response"))?;
        if let ExecutionEvent::CallArgument { word, value } = event {
            if *word != p.words || *word >= declaration.argument_words {
                return Err(integrity("call argument order differs"));
            }
            p.arguments[usize::from(*word)] = *value;
            p.words += 1;
            return Ok(());
        }
        if p.words != declaration.argument_words {
            return Err(integrity("missing modeled call arguments"));
        }
        match event {
            ExecutionEvent::Allocation {
                address,
                requested,
                capacity,
                lifetime,
            } => {
                let a = r
                    .allocation
                    .ok_or_else(|| integrity("undeclared call allocation"))?;
                if p.allocation
                    || p.outputs != 0
                    || p.delay
                    || *address != a.address
                    || *capacity != a.capacity
                    || *lifetime != a.lifetime
                    || p.arguments[usize::from(a.size_argument)] != Some(*requested)
                    || requested > capacity
                {
                    return Err(integrity("allocation differs from response"));
                }
                p.allocation = true;
            }
            ExecutionEvent::CallOutput {
                address,
                width,
                value,
                scope,
            } => {
                let o = r
                    .outputs
                    .get(p.outputs)
                    .ok_or_else(|| integrity("extra model output"))?;
                if p.allocation != r.allocation.is_some()
                    || p.delay
                    || *width != o.width
                    || *value != o.value
                    || *scope != o.scope
                    || p.arguments[usize::from(o.pointer_argument)]
                        .and_then(|a| a.checked_add(o.byte_offset))
                        != Some(*address)
                {
                    return Err(integrity("model output differs from response"));
                }
                p.outputs += 1;
            }
            ExecutionEvent::DelayMicros { value } => {
                let expected = match r.delay_micros {
                    None => None,
                    Some(CallValue::Constant { value }) => Some(value),
                    Some(CallValue::Argument { word }) => p.arguments[usize::from(word)],
                };
                if p.delay
                    || p.outputs != r.outputs.len()
                    || p.allocation != r.allocation.is_some()
                    || expected != Some(*value)
                {
                    return Err(integrity("model delay differs from response"));
                }
                p.delay = true;
            }
            ExecutionEvent::CallReturn { words } => {
                if *words != r.return_words
                    || p.outputs != r.outputs.len()
                    || p.allocation != r.allocation.is_some()
                    || p.delay != r.delay_micros.is_some()
                {
                    return Err(integrity("model return has missing/different effects"));
                }
                self.live[p.slot].calls += 1;
                self.pending = None;
            }
            _ => return Err(integrity("non-model event interrupts response trace")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_trace_effects_returns_and_consumption_cannot_be_forged() {
        let d = CallDeclaration {
            repetition: blobray_domain::CallRepetition::Finite,
            id: "fixture".into(),
            applicability: "fixture".into(),
            lifetime: RegionLifetime::Phase,
            binding: CallBinding {
                address: 0x2000,
                boundary: CallBoundary::Unmapped,
                allow_tail: false,
            },
            argument_words: 1,
            responses: vec![CallResponse {
                return_words: [Some(7), None],
                outputs: vec![CallOutput {
                    pointer_argument: 0,
                    byte_offset: 4,
                    width: 4,
                    value: 42,
                    scope: CallOutputScope::NormalMemory,
                }],
                allocation: None,
                delay_micros: Some(CallValue::Constant { value: 5 }),
            }],
        };
        let events = [
            ExecutionEvent::ModeledCall {
                site: 0x1000,
                target: 0x2000,
                tail: false,
                response: 0,
                boundary: CallBoundary::Unmapped,
            },
            ExecutionEvent::CallArgument {
                word: 0,
                value: Some(0x3000),
            },
            ExecutionEvent::CallOutput {
                address: 0x3004,
                width: 4,
                value: 42,
                scope: CallOutputScope::NormalMemory,
            },
            ExecutionEvent::DelayMicros { value: 5 },
            ExecutionEvent::CallReturn {
                words: [Some(7), None],
            },
        ];
        let observed = CallObservation {
            id: d.id.clone(),
            definition: d.identity(&mut || Ok(())).unwrap(),
            lifetime: d.lifetime,
            calls: 1,
            remaining: 0,
            closed: true,
            issue: None,
            status: ModelStatus::Complete,
        };
        let check = |events: &[ExecutionEvent], o: &CallObservation| -> Result<()> {
            let mut tracker = Calls::new();
            tracker.begin(
                std::slice::from_ref(&d),
                SessionReset::Cold,
                false,
                &mut || Ok(()),
            )?;
            for e in events {
                tracker.event(e, &mut || Ok(()))?;
            }
            tracker.observe(o, true, false, &mut || Ok(()))?;
            tracker.finish_side()?;
            assert!(tracker.closed());
            Ok(())
        };
        check(&events, &observed).unwrap();
        for variant in 0..7 {
            let mut forged = events.to_vec();
            match variant {
                0 => {
                    forged.remove(2);
                }
                1 => {
                    forged[0] = ExecutionEvent::ModeledCall {
                        site: 0x1000,
                        target: 0x2004,
                        tail: false,
                        response: 0,
                        boundary: CallBoundary::Unmapped,
                    }
                }
                2 => {
                    forged[1] = ExecutionEvent::CallArgument {
                        word: 1,
                        value: Some(0x3000),
                    }
                }
                3 => {
                    forged[2] = ExecutionEvent::CallOutput {
                        address: 0x3000,
                        width: 4,
                        value: 42,
                        scope: CallOutputScope::NormalMemory,
                    }
                }
                4 => forged[3] = ExecutionEvent::DelayMicros { value: 6 },
                5 => {
                    forged[4] = ExecutionEvent::CallReturn {
                        words: [Some(8), None],
                    }
                }
                6 => {
                    forged.pop();
                }
                _ => unreachable!(),
            }
            assert_eq!(
                check(&forged, &observed).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        let mut forged = observed.clone();
        forged.definition = ArtifactId::of_bytes(b"other");
        assert_eq!(
            check(&events, &forged).unwrap_err().code,
            ErrorCode::Integrity
        );
        forged = observed;
        forged.closed = false;
        forged.status = ModelStatus::Open;
        assert_eq!(
            check(&events, &forged).unwrap_err().code,
            ErrorCode::Integrity
        );
    }
}
