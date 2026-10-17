//! Sequential waits within one TX DC/PWDET calibration, excluding cold callers.
use super::{Bus, Event, Kind, Timing};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Pbus,
    Search,
    Tone,
    Sar,
    Root,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub pbus: Bus,
    pub search: Timing,
    pub tone: Timing,
    pub sar: Timing,
    pub root: Timing,
    /// Existing status samples; neither extra reads nor physical timestamps.
    pub sar_ready: u16,
    pub sar_not_ready: u16,
}

#[derive(Default)]
pub(crate) struct Recorder {
    report: Report,
    active: Option<(Scope, Kind, u64)>,
    invalid: bool,
}

impl Recorder {
    pub(crate) fn observe(&mut self, scope: Scope, kind: Kind, event: Event) {
        if (scope == Scope::Pbus) == (kind == Kind::Settle) {
            self.invalid = true;
            return;
        }
        match event {
            Event::Started { requested_micros } => {
                if self.active.is_some() {
                    self.invalid = true;
                    return;
                }
                self.active = Some((scope, kind, requested_micros));
            }
            Event::Completed {
                elapsed_micros,
                lateness_micros,
            } => {
                let Some((started_scope, started_kind, requested)) = self.active.take() else {
                    self.invalid = true;
                    return;
                };
                if (scope, kind) != (started_scope, started_kind)
                    || elapsed_micros.checked_sub(requested) != Some(lateness_micros)
                {
                    self.invalid = true;
                    return;
                }
                let timing = match scope {
                    Scope::Pbus => {
                        if kind == Kind::BusBusy {
                            let Some(next) = self.report.pbus.bus_busy.checked_add(1) else {
                                self.invalid = true;
                                return;
                            };
                            self.report.pbus.bus_busy = next;
                        }
                        &mut self.report.pbus.timing
                    }
                    Scope::Search => &mut self.report.search,
                    Scope::Tone => &mut self.report.tone,
                    Scope::Sar => &mut self.report.sar,
                    Scope::Root => &mut self.report.root,
                };
                let Ok(requested) = u32::try_from(requested) else {
                    self.invalid = true;
                    return;
                };
                let Ok(elapsed) = u32::try_from(elapsed_micros) else {
                    self.invalid = true;
                    return;
                };
                let Ok(lateness) = u32::try_from(lateness_micros) else {
                    self.invalid = true;
                    return;
                };
                let (Some(count), Some(requested), Some(elapsed)) = (
                    timing.count.checked_add(1),
                    timing.requested_micros.checked_add(requested),
                    timing.elapsed_micros.checked_add(elapsed),
                ) else {
                    self.invalid = true;
                    return;
                };
                *timing = Timing {
                    count,
                    requested_micros: requested,
                    elapsed_micros: elapsed,
                    maximum_lateness_micros: timing.maximum_lateness_micros.max(lateness),
                };
            }
        }
    }

    pub(crate) fn sar_ready(&mut self, ready: bool) {
        let count = if ready {
            &mut self.report.sar_ready
        } else {
            &mut self.report.sar_not_ready
        };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.invalid = true,
        }
    }
    pub(crate) fn report(&self) -> Report {
        self.report
    }
    pub(crate) fn is_complete(&self) -> bool {
        !self.invalid && self.active.is_none()
    }
}

#[cfg(test)]
mod tests;
