//! Durable completion of one scenario's whole repetition set: the seal.

use super::*;
use oer_tool_data_hil_run_bundle_format::run::{
    ATTEMPT_SEAL_SCHEMA, ATTEMPTS, AttemptSeal, material_files,
};
use oer_tool_service_hil_scenario::{Scenario, ScenarioFamily};

impl RunSession {
    pub fn seal_scenario<F: ScenarioFamily>(
        &mut self,
        scenario: &Scenario<F>,
        result: &ScenarioResult,
    ) -> Result<()> {
        let image = scenario.image();
        if result.scenario != scenario.id()
            || result.image != image
            || result.required_repetitions != scenario.repetitions()
        {
            return Err("attempt result does not match its selected scenario".into());
        }
        let seal_path = self
            .directory
            .join(ATTEMPTS)
            .join(format!("{}.json", scenario.id()));
        if seal_path.try_exists()? {
            return Err("a sealed scenario attempt cannot be replaced".into());
        }
        let mut manifest = self.manifest.clone();
        manifest.state = RunState::Completed;
        manifest.finished_unix_millis = Some(unix_millis());
        manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        // Other images may be prepared later. They are not this experiment's
        // subject, and their mutable manifest must not invalidate this seal.
        manifest.firmware.retain(|artifact| artifact.image == image);
        let suite = SuiteResult {
            schema: RUN_SCHEMA,
            run_id: manifest.run_id.clone(),
            target: manifest.target.clone(),
            outcome: if result.outcome.is_passed() {
                Outcome::Passed
            } else {
                Outcome::Failed
            },
            started_unix_millis: manifest.started_unix_millis,
            finished_unix_millis: manifest.finished_unix_millis.unwrap(),
            duration_millis: manifest.duration_millis.unwrap(),
            counts: SuiteCounts::from_results(std::slice::from_ref(result)),
            scenarios: vec![result.clone()],
        };
        validation::validate_suite(&suite, &manifest)?;
        let output = self.scenario_directory(scenario.id());
        atomic_json(&output.join("scenario.json"), scenario)?;
        atomic_json(&output.join("result.json"), result)?;
        let files = material_files(&self.directory, &manifest, scenario.id(), image)?;
        atomic_json(
            &seal_path,
            &AttemptSeal {
                schema: ATTEMPT_SEAL_SCHEMA,
                manifest,
                suite,
                files,
            },
        )
    }
}
