//! Physical call boundaries named by execution contracts; names never resolve executable identities.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallEndpoint {
    /// The standalone executable, by content, whose code holds the boundary.
    pub object: ObjectId,
    /// The exact symbol at a code boundary; none at a call model.
    pub symbol: Option<SymbolId>,
    pub boundary: ReviewedCallBoundary,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ReviewedCallBoundary {
    /// The endpoint must name the exact symbol, including its table identity.
    Code { address: u32 },
    Model {
        binding: CallBinding,
        definition: ArtifactId,
    },
}
impl CallEndpoint {
    pub fn address(&self) -> u32 {
        match self.boundary {
            ReviewedCallBoundary::Code { address } => address,
            ReviewedCallBoundary::Model { binding, .. } => binding.address,
        }
    }
    pub fn target_kind(&self) -> ObservedCallTarget {
        match self.boundary {
            ReviewedCallBoundary::Code { .. } => ObservedCallTarget::CapturedCode,
            ReviewedCallBoundary::Model { .. } => ObservedCallTarget::CallModel,
        }
    }
    pub fn in_target(&self, target: &ExecutionTarget) -> bool {
        target.maps(&self.object)
    }
    pub fn validate(&self) -> Result<()> {
        if self.address() & 1 != 0
            || self.address() >= u32::MAX - 1
            || self.object.location != ObjectLocation::Standalone
            || self
                .symbol
                .as_ref()
                .is_some_and(|s| s.object != self.object)
            || matches!(self.boundary, ReviewedCallBoundary::Code { .. }) != self.symbol.is_some()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid reviewed call endpoint",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.object.artifact.allocated_bytes()
            + self
                .symbol
                .as_ref()
                .map_or(0, |s| s.object.artifact.allocated_bytes())
            + match &self.boundary {
                ReviewedCallBoundary::Code { .. } => 0,
                ReviewedCallBoundary::Model { definition, .. } => definition.allocated_bytes(),
            }
    }
}
