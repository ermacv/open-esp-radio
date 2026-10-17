use core::fmt;

/// Maximum MAC bytes when the two-byte FCS occupies the remainder of a
/// standard 127-byte IEEE 802.15.4 PSDU.
pub const MAX_MAC_FRAME_LEN: usize = 125;

/// Smallest MAC byte sequence accepted by the portable transport.
pub const MIN_MAC_FRAME_LEN: usize = 1;

const ACKNOWLEDGEMENT_REQUEST_BIT: u8 = 0x20;

/// A MAC byte sequence cannot be represented by [`Frame`] or [`FrameView`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// An empty slice is not a transmittable MAC frame.
    Empty,
    /// The byte sequence exceeds the standard FCS-reserving MAC capacity.
    TooLong {
        /// Supplied byte count.
        length: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
}

/// Borrowed MAC bytes with no PHR, FCS reservation or DMA metadata.
///
/// The private field prevents a caller from bypassing length validation.
/// The borrow also prevents a view from escaping its packet storage:
///
/// ```compile_fail
/// use oer_ieee802154::FrameView;
///
/// let escaped = {
///     let bytes = [0x41, 0x88, 0x00];
///     FrameView::new(&bytes).unwrap()
/// };
/// let _ = escaped.bytes();
/// ```
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct FrameView<'frame> {
    bytes: &'frame [u8],
}

impl<'frame> FrameView<'frame> {
    /// Validate borrowed MAC bytes.
    pub const fn new(bytes: &'frame [u8]) -> Result<Self, FrameError> {
        if bytes.is_empty() {
            return Err(FrameError::Empty);
        }
        if bytes.len() > MAX_MAC_FRAME_LEN {
            return Err(FrameError::TooLong {
                length: bytes.len(),
                maximum: MAX_MAC_FRAME_LEN,
            });
        }
        Ok(Self { bytes })
    }

    /// Return the complete MAC bytes.
    pub const fn bytes(self) -> &'frame [u8] {
        self.bytes
    }

    /// Return the validated byte count.
    pub const fn len(self) -> usize {
        self.bytes.len()
    }

    /// A validated view is never empty.
    pub const fn is_empty(self) -> bool {
        false
    }

    /// Return whether the frame-control field requests an acknowledgement.
    ///
    /// The ACK requirement is part of the immutable frame itself. Portable
    /// requests therefore cannot carry a second caller-selected value that
    /// disagrees with this bit.
    pub const fn acknowledgement_requested(self) -> bool {
        self.bytes[0] & ACKNOWLEDGEMENT_REQUEST_BIT != 0
    }
}

impl fmt::Debug for FrameView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameView")
            .field("length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Owned, fixed-capacity MAC bytes.
///
/// Storage has one compile-time capacity and never allocates. Trailing bytes
/// are cleared when a shorter frame replaces an older frame, so adapters do
/// not accidentally retain packet contents outside the declared length.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Frame {
    bytes: [u8; MAX_MAC_FRAME_LEN],
    length: u8,
}

impl Frame {
    /// Copy one validated MAC frame into owned bounded storage.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, FrameError> {
        let view = FrameView::new(bytes)?;
        let mut storage = [0; MAX_MAC_FRAME_LEN];
        storage[..view.len()].copy_from_slice(view.bytes());
        Ok(Self {
            bytes: storage,
            length: view.len() as u8,
        })
    }

    /// Replace the complete frame without modifying `self` on validation
    /// failure.
    pub fn replace(&mut self, bytes: &[u8]) -> Result<(), FrameError> {
        let view = FrameView::new(bytes)?;
        self.bytes.fill(0);
        self.bytes[..view.len()].copy_from_slice(view.bytes());
        self.length = view.len() as u8;
        Ok(())
    }

    /// Borrow the complete MAC bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Borrow a validated portable view.
    pub fn view(&self) -> FrameView<'_> {
        FrameView {
            bytes: self.as_bytes(),
        }
    }

    /// Return the MAC byte count.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// An owned frame is constructed only from a non-empty view.
    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl<'frame> From<&'frame Frame> for FrameView<'frame> {
    fn from(frame: &'frame Frame) -> Self {
        frame.view()
    }
}

impl TryFrom<&[u8]> for Frame {
    type Error = FrameError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::try_from_bytes(bytes)
    }
}

#[cfg(test)]
mod tests;
