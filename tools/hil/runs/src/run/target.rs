//! What a run is for: its chip and its manifest so far.

use super::*;

impl RunSession {
    /// Where the run's chip keeps its builds and archived objects.
    pub fn target_directory(&self) -> &Path {
        &self.target_directory
    }

    /// The chip this run is for.
    pub fn target(&self) -> &str {
        &self.manifest.target
    }

    /// The run's manifest as recorded so far.
    pub fn manifest(&self) -> &RunManifest {
        &self.manifest
    }
}
