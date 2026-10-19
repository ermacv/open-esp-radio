//! Bind one complete source snapshot before any experiment can finish.

use super::*;

impl RunSession {
    /// Bind the snapshot in `directory`, whose files are in `objects`,
    /// archived into this run and checked out in a free build workspace of
    /// `build_slots`.
    pub fn bind_source_snapshot(
        &mut self,
        directory: &Path,
        objects: &Path,
        build_slots: &Path,
    ) -> Result<()> {
        if self.frozen_sources.is_some()
            || !self.manifest.firmware.is_empty()
            || self.directory.join("attempts").exists()
        {
            return Err("source snapshot must be bound once, before firmware or attempts".into());
        }
        let (frozen, sources, files) = crate::build::archive_snapshot(
            directory,
            objects,
            &self.directory,
            &self.target_directory,
            build_slots,
        )?;
        let primary = sources
            .first()
            .filter(|s| s.name == "repository")
            .ok_or("snapshot must start with the primary repository")?;
        self.manifest.repository = RepositoryProvenance {
            commit: primary.commit.clone(),
            dirty: primary.dirty,
            workspace_sha256: primary.workspace_sha256.clone(),
        };
        self.source_materials = sources;
        self.snapshot_materials = files;
        self.frozen_sources = Some(frozen);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        self.record_event(RunEventKind::SourceSnapshotBound, None, None, None)
    }

    /// The sources this run builds from, once bound.
    pub fn frozen_sources(&self) -> Option<&oer_hil_source_snapshot::FrozenSources> {
        self.frozen_sources.as_ref()
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
