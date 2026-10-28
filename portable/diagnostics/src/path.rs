//! The variant path of a nested error: the names of its variants from the
//! outermost to the innermost, and the numbers the innermost carries. A type
//! describes itself with an exhaustive `match` over its own variants, so a
//! new variant breaks the build instead of printing nothing.

/// Deepest chain of variant names one path keeps.
pub const MAX_PATH_NAMES: usize = 6;
/// Most numbers one path keeps.
pub const MAX_PATH_WORDS: usize = 4;

const _: () = {
    if MAX_PATH_NAMES > u8::MAX as usize || MAX_PATH_WORDS > u8::MAX as usize {
        panic!("path capacities fit their counters");
    }
};

/// What a value contributes to a note's path.
pub trait DiagnosticPath {
    /// Pushes this value's variant name, then whatever its payload adds.
    fn push_path(&self, path: &mut DiagPath);
}

impl<T: DiagnosticPath, E: DiagnosticPath> DiagnosticPath for Result<T, E> {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Ok(value) => {
                path.name("Ok");
                value.push_path(path);
            }
            Err(error) => {
                path.name("Err");
                error.push_path(path);
            }
        }
    }
}

/// A path being built. Anything that does not fit is not dropped silently:
/// the path says it overflowed, and the host prints that.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagPath {
    names: [&'static str; MAX_PATH_NAMES],
    name_count: u8,
    words: [u32; MAX_PATH_WORDS],
    word_count: u8,
    overflow: bool,
}

impl DiagPath {
    pub const fn new() -> Self {
        Self {
            names: [""; MAX_PATH_NAMES],
            name_count: 0,
            words: [0; MAX_PATH_WORDS],
            word_count: 0,
            overflow: false,
        }
    }

    /// The path of `value`.
    pub fn of(value: &impl DiagnosticPath) -> Self {
        let mut path = Self::new();
        value.push_path(&mut path);
        path
    }

    pub fn name(&mut self, name: &'static str) {
        match self.names.get_mut(usize::from(self.name_count)) {
            Some(slot) => {
                *slot = name;
                // A successful slot lookup bounds the count by MAX_PATH_NAMES.
                self.name_count = self.name_count.wrapping_add(1);
            }
            None => self.overflow = true,
        }
    }

    pub fn word(&mut self, word: u32) {
        match self.words.get_mut(usize::from(self.word_count)) {
            Some(slot) => {
                *slot = word;
                // A successful slot lookup bounds the count by MAX_PATH_WORDS.
                self.word_count = self.word_count.wrapping_add(1);
            }
            None => self.overflow = true,
        }
    }

    /// Marks that the value had more than the path keeps.
    pub fn truncated(&mut self) {
        self.overflow = true;
    }

    /// A 64-bit number as two words, low word first.
    pub fn wide(&mut self, value: u64) {
        self.word(value as u32);
        self.word((value >> 32) as u32);
    }

    pub fn names(&self) -> &[&'static str] {
        self.names
            .get(..usize::from(self.name_count))
            .unwrap_or(&[])
    }

    pub fn words(&self) -> &[u32] {
        self.words
            .get(..usize::from(self.word_count))
            .unwrap_or(&[])
    }

    /// Whether something did not fit.
    pub const fn overflowed(&self) -> bool {
        self.overflow
    }
}
