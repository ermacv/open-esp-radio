//! An application's line ([`Topic::Line`]): static text pieces and typed
//! values in their order, and at most one error path at the end. The
//! firmware builds no text: a piece is a `&'static str` the note carries
//! as a name, a value is a word, and the note's first word, its shape,
//! says what each segment is. The host prints the pieces and the values
//! in that order.
//!
//! What does not fit the note (more than [`MAX_SEGMENTS`] segments, a piece
//! or a value past the note's capacity, a segment after the path) is left
//! out, and the note says it overflowed; the host prints `line-truncated`.

use crate::note::{MAX_NAME, MAX_NAME_BYTES, MAX_NAMES, MAX_WORDS, Note, Topic};
use crate::{DiagPath, DiagnosticPath};

/// Most segments one line describes: its shape word holds a 4-bit code
/// for each.
pub const MAX_SEGMENTS: usize = 8;

/// The kind of one segment of a line, as its shape word encodes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Segment {
    /// A static text piece: the next name.
    Text = 1,
    /// An unsigned decimal: one word.
    Dec = 2,
    /// A `0x`-prefixed hexadecimal: one word.
    Hex = 3,
    /// A signed decimal: one word.
    Signed = 4,
    /// `true` or `false`: one word.
    Flag = 5,
    /// An IPv4 address, most significant byte first: one word.
    Ipv4 = 6,
    /// A 64-bit decimal: two words, low first.
    Wide = 7,
    /// An error's variant path: every remaining name and word. Last.
    Path = 8,
}

impl Segment {
    const fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::Text,
            2 => Self::Dec,
            3 => Self::Hex,
            4 => Self::Signed,
            5 => Self::Flag,
            6 => Self::Ipv4,
            7 => Self::Wide,
            8 => Self::Path,
            _ => return None,
        })
    }

    const fn words(self) -> usize {
        match self {
            Self::Text | Self::Path => 0,
            Self::Wide => 2,
            _ => 1,
        }
    }
}

/// What a shape word asks of its note: the segments in order, the words
/// and names the segments before a path take, and whether a path ends it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shape {
    segments: [Option<Segment>; MAX_SEGMENTS],
    pub words: usize,
    pub names: usize,
    pub path: bool,
}

impl Shape {
    /// The shape `word` describes, or `None` for an unknown code, a code
    /// after the end, or a segment after the path.
    pub fn of(word: u32) -> Option<Self> {
        let mut shape = Self {
            segments: [None; MAX_SEGMENTS],
            words: 0,
            names: 0,
            path: false,
        };
        let mut ended = false;
        for (index, slot) in shape.segments.iter_mut().enumerate() {
            let code = word
                .checked_shr(index.saturating_mul(4) as u32)
                .unwrap_or(0)
                & 0xf;
            if code == 0 {
                ended = true;
                continue;
            }
            if ended || shape.path {
                return None;
            }
            let segment = Segment::from_code(code)?;
            *slot = Some(segment);
            // At most eight segments of at most two words or one name.
            shape.words = shape.words.saturating_add(segment.words());
            if matches!(segment, Segment::Text) {
                shape.names = shape.names.saturating_add(1);
            }
            shape.path = matches!(segment, Segment::Path);
        }
        Some(shape)
    }

    /// The segments, in order.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.segments.iter().map_while(|segment| *segment)
    }

    /// Whether a note of `words` value words and `names` names has the
    /// values and pieces this shape asks for: exactly, or at least with a
    /// path, which takes the rest.
    pub const fn fits(&self, words: usize, names: usize) -> bool {
        if self.path {
            words >= self.words && names >= self.names
        } else {
            words == self.words && names == self.names
        }
    }
}

/// A line being built: `Line::new().text("ipv4 ").value(&Ipv4(address))`.
/// [`Line::note`] gives the note to write.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    note: Note<'static>,
    segments: usize,
    closed: bool,
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl Line {
    pub fn new() -> Self {
        let mut note = Note::new(Topic::Line);
        // The shape word.
        note.count = 1;
        Self {
            note,
            segments: 0,
            closed: false,
        }
    }

    /// Appends `segment` to the shape when the line has room for it and is
    /// not closed by a path; otherwise marks the overflow.
    fn segment(&mut self, segment: Segment) -> bool {
        let Some(shift) = (self.segments < MAX_SEGMENTS)
            .then(|| self.segments.saturating_mul(4))
            .filter(|_| !self.closed)
        else {
            self.note.overflow = true;
            return false;
        };
        if let Some(shape) = self.note.words.first_mut() {
            *shape |= u32::from(segment as u8) << shift;
        }
        // Bounded by MAX_SEGMENTS above.
        self.segments = self.segments.saturating_add(1);
        true
    }

    /// A static text piece.
    pub fn text(mut self, text: &'static str) -> Self {
        let names = usize::from(self.note.name_count);
        let bytes = usize::from(self.note.name_bytes).saturating_add(text.len());
        if names >= MAX_NAMES || text.len() > MAX_NAME || bytes > MAX_NAME_BYTES {
            self.note.overflow = true;
        } else if self.segment(Segment::Text) {
            if let Some(slot) = self.note.names.get_mut(names) {
                *slot = text;
            }
            // Bounded by MAX_NAMES and MAX_NAME_BYTES, both below 256.
            self.note.name_count = self.note.name_count.saturating_add(1);
            self.note.name_bytes = bytes as u8;
        }
        self
    }

    /// A value: a number, a flag, an address or an error's path.
    pub fn value(self, value: &(impl LineValue + ?Sized)) -> Self {
        value.push(self)
    }

    /// `words`, as a segment of `segment`'s kind.
    fn words(mut self, segment: Segment, words: &[u32]) -> Self {
        let count = usize::from(self.note.count);
        let end = count.saturating_add(words.len());
        if end > MAX_WORDS {
            self.note.overflow = true;
        } else if self.segment(segment) {
            for (slot, word) in self.note.words.iter_mut().skip(count).zip(words) {
                *slot = *word;
            }
            // Bounded by MAX_WORDS.
            self.note.count = end as u8;
        }
        self
    }

    /// The error path that ends the line: its variants and numbers take the
    /// note's remaining names and words.
    pub fn path(mut self, value: &(impl DiagnosticPath + ?Sized)) -> Self {
        if self.segment(Segment::Path) {
            self.closed = true;
            let mut path = DiagPath::new();
            value.push_path(&mut path);
            self.note = self.note.pathed(&path);
        }
        self
    }

    /// The note of the line.
    pub const fn note(&self) -> Note<'static> {
        self.note
    }
}

/// A value a line prints: its segment kind and words, or a path.
pub trait LineValue {
    fn push(&self, line: Line) -> Line;
}

macro_rules! unsigned {
    ($($type:ty),*) => {$(
        impl LineValue for $type {
            fn push(&self, line: Line) -> Line {
                line.words(Segment::Dec, &[u32::from(*self)])
            }
        }
    )*};
}
unsigned!(u8, u16, u32);

macro_rules! signed {
    ($($type:ty),*) => {$(
        impl LineValue for $type {
            fn push(&self, line: Line) -> Line {
                // The host reads the word back as an i32.
                line.words(Segment::Signed, &[i32::from(*self) as u32])
            }
        }
    )*};
}
signed!(i8, i16, i32);

impl LineValue for u64 {
    fn push(&self, line: Line) -> Line {
        line.words(Segment::Wide, &[*self as u32, (*self >> 32) as u32])
    }
}

impl LineValue for usize {
    fn push(&self, line: Line) -> Line {
        (*self as u64).push(line)
    }
}

impl LineValue for bool {
    fn push(&self, line: Line) -> Line {
        line.words(Segment::Flag, &[u32::from(*self)])
    }
}

/// A static text value, such as a variant's name.
impl LineValue for &'static str {
    fn push(&self, line: Line) -> Line {
        line.text(self)
    }
}

/// A number printed as `0x`-prefixed hexadecimal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hex(pub u32);

impl LineValue for Hex {
    fn push(&self, line: Line) -> Line {
        line.words(Segment::Hex, &[self.0])
    }
}

/// An IPv4 address, printed dotted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4(pub [u8; 4]);

impl LineValue for Ipv4 {
    fn push(&self, line: Line) -> Line {
        line.words(Segment::Ipv4, &[u32::from_be_bytes(self.0)])
    }
}

/// An error: its variant path ends the line.
impl<T: DiagnosticPath + ?Sized> LineValue for T {
    fn push(&self, line: Line) -> Line {
        line.path(self)
    }
}

#[cfg(test)]
mod tests;
