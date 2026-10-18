//! Preserve independent observations before interpreting qualification gates.

use serde::Serialize;
use std::{collections::BTreeMap, fmt::Display};

#[derive(Serialize)]
pub(super) struct CycleProgress {
    cycle: u8,
    stages: BTreeMap<&'static str, serde_json::Value>,
}

impl CycleProgress {
    pub(super) fn new(cycle: u8) -> Self {
        Self {
            cycle,
            stages: BTreeMap::new(),
        }
    }

    pub(super) fn record<T: Serialize, E: Display>(
        &mut self,
        stage: &'static str,
        result: &Result<T, E>,
    ) {
        let value = match result {
            Ok(value) => serde_json::json!({"status": "available", "value": value}),
            Err(error) => serde_json::json!({"status": "error", "error": error.to_string()}),
        };
        self.stages.insert(stage, value);
    }

    /// Keep this cycle's progress among the repetition's observations.
    pub(super) fn save(&self, results: &oer_hil_workload::results::Results) {
        results.replace(format!("cycle-{:03}-progress", self.cycle), self);
    }
}

#[cfg(test)]
mod tests;
