//! Session-owned external-call declarations and response cursors.
use crate::*;
struct Instance<'m> {
    declaration: CallDeclaration,
    definition: ArtifactId,
    _payload: MemoryReservation<'m>,
    consumed: u32,
    issue: Option<CallIssue>,
}
pub(crate) struct Calls<'m> {
    memory: &'m WorkingMemory,
    instances: AdmittedVec<'m, Option<Instance<'m>>>,
    index: AdmittedVec<'m, (u32, usize)>,
}
impl<'m> Calls<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            instances: AdmittedVec::new(memory),
            index: AdmittedVec::new(memory),
        }
    }
    pub fn install(
        &mut self,
        declarations: &[CallDeclaration],
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for d in declarations {
            c.checkpoint(self.instances.len() as u64 + 1)?;
            d.validate()?;
            if self.instances.iter().flatten().any(|i| {
                i.declaration.id == d.id || i.declaration.binding.address == d.binding.address
            }) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "external-call id/address already live; omit it on warm continuation",
                ));
            }
            let payload = self.memory.reserve(d.payload_bytes() + 64, c.position())?;
            let instance = Instance {
                definition: d.identity(c)?,
                declaration: d.clone(),
                _payload: payload,
                consumed: 0,
                issue: None,
            };
            if let Some(slot) = self.instances.iter().position(Option::is_none) {
                self.instances[slot] = Some(instance);
            } else {
                if self.instances.len() == MAX_CALL_MODELS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "live call model capacity exhausted",
                    ));
                }
                self.instances.push(Some(instance), c.position())?;
            }
        }
        self.rebuild(c)
    }
    fn rebuild(&mut self, c: &mut dyn RunControl) -> Result<()> {
        while self.index.pop().is_some() {}
        for (slot, i) in self.instances.iter().enumerate() {
            c.checkpoint(1)?;
            if let Some(i) = i {
                self.index
                    .push((i.declaration.binding.address, slot), c.position())?;
            }
        }
        c.checkpoint(self.index.len() as u64 * (self.index.len().max(1).ilog2() as u64 + 1))?;
        self.index.sort_unstable_by_key(|i| i.0);
        Ok(())
    }
    pub fn find(&self, address: u32, c: &mut dyn RunControl) -> Result<Option<usize>> {
        c.checkpoint(self.index.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .index
            .binary_search_by_key(&address, |i| i.0)
            .ok()
            .map(|i| self.index[i].1))
    }
    pub fn bindings(&self) -> impl Iterator<Item = CallBinding> + '_ {
        self.instances
            .iter()
            .flatten()
            .map(|i| i.declaration.binding)
    }
    pub fn declaration(&self, slot: usize) -> &CallDeclaration {
        &self.instances[slot].as_ref().unwrap().declaration
    }
    pub fn consumed(&self, slot: usize) -> u32 {
        self.instances[slot].as_ref().unwrap().consumed
    }
    pub fn response(&self, slot: usize) -> Option<&CallResponse> {
        let i = self.instances[slot].as_ref().unwrap();
        i.declaration.response(i.consumed)
    }
    pub fn fail(&mut self, slot: usize, issue: CallIssue) -> CallDispatch {
        self.instances[slot].as_mut().unwrap().issue = Some(issue);
        CallDispatch::Incomplete { issue }
    }
    pub fn consume(&mut self, slot: usize) {
        self.instances[slot].as_mut().unwrap().consumed += 1;
    }
    pub fn finish(
        &mut self,
        close_chain: bool,
        output: &mut Vec<CallObservation>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        output.clear();
        for slot in &mut *self.instances {
            let Some(i) = slot else { continue };
            c.checkpoint(1)?;
            let closed = close_chain || i.declaration.lifetime == RegionLifetime::Phase;
            let mut observed = CallObservation {
                id: i.declaration.id.clone(),
                definition: i.definition.clone(),
                lifetime: i.declaration.lifetime,
                calls: i.consumed,
                remaining: i.declaration.remaining(i.consumed),
                closed,
                issue: i.issue,
                status: ModelStatus::Open,
            };
            observed.status = observed.expected_status();
            output.push(observed);
            if closed {
                *slot = None;
            }
        }
        self.rebuild(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_owners_release_on_closure_and_failed_admission() {
        let mut d = CallDeclaration {
            repetition: blobray_domain::CallRepetition::Finite,
            id: "fixture".into(),
            applicability: "lifecycle".into(),
            lifetime: RegionLifetime::Session,
            binding: CallBinding {
                address: 0x2000,
                boundary: CallBoundary::Unmapped,
                allow_tail: false,
            },
            argument_words: 0,
            responses: vec![
                CallResponse {
                    return_words: [Some(7), None],
                    outputs: vec![],
                    allocation: None,
                    delay_micros: None
                };
                128
            ],
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut calls = Calls::new(&memory);
        let mut output = Vec::with_capacity(MAX_CALL_MODELS);
        calls.install(&[d.clone()], &mut || Ok(())).unwrap();
        let live = memory.observation().reserved_bytes;
        calls.consume(0);
        calls.finish(false, &mut output, &mut || Ok(())).unwrap();
        assert_eq!(memory.observation().reserved_bytes, live);
        assert_eq!(output[0].calls, 1);
        calls.finish(true, &mut output, &mut || Ok(())).unwrap();
        let released = memory.observation().reserved_bytes;
        assert!(live > released + d.payload_bytes());
        assert_eq!(output[0].status, ModelStatus::Incomplete);
        d.lifetime = RegionLifetime::Phase;
        calls.install(&[d.clone()], &mut || Ok(())).unwrap();
        calls.finish(false, &mut output, &mut || Ok(())).unwrap();
        assert_eq!(memory.observation().reserved_bytes, released);
        drop(calls);
        assert_eq!(memory.observation().reserved_bytes, 0);
        let memory = WorkingMemory::new(1024).unwrap();
        let mut calls = Calls::new(&memory);
        assert_eq!(
            calls
                .install(&[d.clone()], &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(memory.observation().reserved_bytes, 0);
        assert_eq!(
            calls
                .install(&[d], &mut || Err(Error::new(
                    ErrorCode::Cancelled,
                    "fixture"
                )))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
}
