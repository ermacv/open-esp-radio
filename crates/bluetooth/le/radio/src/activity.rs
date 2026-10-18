//! Which long-lived roles are active, for radios that share the air.

/// The roles a controller core has active.
///
/// A role is active from the moment the Host enables it until the core has
/// stopped it: advertising and scanning while enabled or still stopping, a
/// connection from its creation until it closes. Radios that share the
/// antenna with other protocols publish this summary to their coexistence
/// arbiter; it carries no timing and no event identities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RadioActivity {
    /// An advertising set is active.
    pub advertising: bool,
    /// The scanner is active.
    pub scanning: bool,
    /// A connection is active.
    pub connected: bool,
}

impl RadioActivity {
    /// No role active.
    pub const IDLE: Self = Self {
        advertising: false,
        scanning: false,
        connected: false,
    };
}
