//! The image-key gate of a workload: the image must serve what it drives.

use std::time::Duration;

use oer_hil_link::SerialCapture;
use oer_hil_protocol::{DeviceImageKeys, Message};

use crate::Result;

/// How long a workload waits for the image keys of the boot it captured.
pub const IMAGE_KEYS_TIMEOUT: Duration = Duration::from_secs(10);

/// The image keys of the captured boot, when its image serves `M`; an error
/// naming `M` otherwise. Further keys a workload needs it checks with
/// [`DeviceImageKeys::require`].
pub fn require_keys<M: Message>(capture: &SerialCapture) -> Result<DeviceImageKeys> {
    let keys = capture.request_image_keys(IMAGE_KEYS_TIMEOUT)?;
    if !keys.has::<M>() {
        return Err(format!("firmware does not advertise {}", M::PATH).into());
    }
    Ok(keys)
}
