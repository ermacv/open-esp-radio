//! The HIL image vocabulary: the image classes and the runtime feature
//! deltas of experiments, as runs record them.

mod class;
mod features;

pub use class::ImageClass;
pub use features::FeatureDelta;
