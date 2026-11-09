//! Feature geometry for the parts solve, rebuilt from the library snapshot
//! for every solve and never stored (§5.2, §5.4): a placement's origin and
//! axes, pad centres and sides, courtyard sides and centre (slice S2);
//! authored anchors and 3D model faces, edges and vertices come from the
//! part's model design in the host (slices S2 and S3).
use super::{FeatureGeometry, not_yet};
use crate::diagnostics::Result;
use crate::model::Project;
use crate::model::dimension::FeatureRef;

/// `feature`'s geometry in its target's frame.
pub fn geometry(_project: &Project, _feature: &FeatureRef) -> Result<FeatureGeometry> {
    Err(not_yet())
}
