//! An application's line ([`Topic::Line`]): static text pieces and typed
//! values in their order, and at most one error path at the end. The
//! firmware builds no text: a piece is a `&'static str` the note carries
//! as a name, a value is a word, and the note's first word, its shape,
//! says what each segment is. The host prints the pieces and the values
//! in that order.
//!
//! [`typed_line!`](crate::typed_line) splits a format string such as
//! `"ipv4 {}/{}"` into its pieces when it compiles and checks there the
//! counts of values, segments, words and text against the note. Only an
//! error's path has a length its type does not give: what of it does not
//! fit the note's names is left out, and the host prints `line-truncated`.
//! A line built by hand with [`Line::text`] and [`Line::value`] is checked
//! as it is built, with the same mark.

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

    /// A static text piece; an empty one adds nothing.
    pub fn text(mut self, text: &'static str) -> Self {
        if text.is_empty() {
            return self;
        }
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
    /// The note words its `push` takes; an error path's words are its own
    /// and take what the values leave.
    const WORDS: usize;
    /// Whether the value is an error's path, which ends its line.
    const PATH: bool = false;

    fn push(&self, line: Line) -> Line;
}

macro_rules! unsigned {
    ($($type:ty),*) => {$(
        impl LineValue for $type {
            const WORDS: usize = 1;

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
            const WORDS: usize = 1;

            fn push(&self, line: Line) -> Line {
                // The host reads the word back as an i32.
                line.words(Segment::Signed, &[i32::from(*self) as u32])
            }
        }
    )*};
}
signed!(i8, i16, i32);

impl LineValue for u64 {
    const WORDS: usize = 2;

    fn push(&self, line: Line) -> Line {
        line.words(Segment::Wide, &[*self as u32, (*self >> 32) as u32])
    }
}

impl LineValue for usize {
    const WORDS: usize = 2;

    fn push(&self, line: Line) -> Line {
        (*self as u64).push(line)
    }
}

impl LineValue for bool {
    const WORDS: usize = 1;

    fn push(&self, line: Line) -> Line {
        line.words(Segment::Flag, &[u32::from(*self)])
    }
}

/// A number printed as `0x`-prefixed hexadecimal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hex(pub u32);

impl LineValue for Hex {
    const WORDS: usize = 1;

    fn push(&self, line: Line) -> Line {
        line.words(Segment::Hex, &[self.0])
    }
}

/// An IPv4 address, printed dotted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4(pub [u8; 4]);

impl LineValue for Ipv4 {
    const WORDS: usize = 1;

    fn push(&self, line: Line) -> Line {
        line.words(Segment::Ipv4, &[u32::from_be_bytes(self.0)])
    }
}

/// An error: its variant path ends the line.
impl<T: DiagnosticPath + ?Sized> LineValue for T {
    const WORDS: usize = 0;
    const PATH: bool = true;

    fn push(&self, line: Line) -> Line {
        line.path(self)
    }
}

/// Most text pieces a format holds: one around each of its holes.
const PIECES: usize = MAX_SEGMENTS + 1;

/// Why a format string cannot make a line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatError {
    /// A `{` that does not open `{}`, or a `}` that does not close it. A
    /// value takes no options; [`Hex`] and [`Ipv4`] choose its text.
    Brace,
    /// More holes and non-empty pieces than [`MAX_SEGMENTS`].
    Segments,
    /// More non-empty pieces than the note's names.
    Pieces,
    /// A piece longer than a name, or pieces longer together than the
    /// note's name bytes.
    Bytes,
}

/// A format string split into its text pieces around its `{}` holes, as
/// [`Format::parse`] makes it when the line compiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Format {
    pieces: [&'static str; PIECES],
    holes: usize,
    error: Option<FormatError>,
}

impl Format {
    /// The pieces and holes of `format`, or why it makes no line. A const
    /// item calls it; [`typed_line!`](crate::typed_line) refuses an error
    /// when the line compiles.
    pub const fn parse(format: &'static str) -> Self {
        let mut this = Self {
            pieces: [""; PIECES],
            holes: 0,
            error: None,
        };
        let (mut texts, mut bytes, mut rest) = (0_usize, 0_usize, format);
        loop {
            // The bytes before the next brace.
            let mut tail = rest.as_bytes();
            let mut at = 0_usize;
            while let [byte, after @ ..] = tail {
                if *byte == b'{' || *byte == b'}' {
                    break;
                }
                at = at.saturating_add(1);
                tail = after;
            }
            // `at` stops before an ASCII brace or at the end: a boundary.
            let Some((piece, after)) = rest.split_at_checked(at) else {
                return this.refused(FormatError::Brace);
            };
            if !piece.is_empty() {
                texts = texts.saturating_add(1);
                bytes = bytes.saturating_add(piece.len());
            }
            if piece.len() > MAX_NAME || bytes > MAX_NAME_BYTES {
                return this.refused(FormatError::Bytes);
            }
            if texts > MAX_NAMES {
                return this.refused(FormatError::Pieces);
            }
            if !set(&mut this.pieces, this.holes, piece) {
                return this.refused(FormatError::Segments);
            }
            match after.as_bytes() {
                [] => break,
                [b'{', b'}', ..] => {}
                _ => return this.refused(FormatError::Brace),
            }
            let Some((_, after)) = after.split_at_checked(2) else {
                return this.refused(FormatError::Brace);
            };
            this.holes = this.holes.saturating_add(1);
            rest = after;
        }
        if this.holes.saturating_add(texts) > MAX_SEGMENTS {
            return this.refused(FormatError::Segments);
        }
        this
    }

    const fn refused(mut self, error: FormatError) -> Self {
        self.error = Some(error);
        self
    }

    /// Why the format makes no line.
    pub const fn error(&self) -> Option<FormatError> {
        self.error
    }

    /// The `{}` holes: one value each.
    pub const fn holes(&self) -> usize {
        self.holes
    }

    /// Whether no text follows the last hole, as an error's path needs.
    pub const fn ends_open(&self) -> bool {
        let mut slots: &[&str] = &self.pieces;
        let mut index = self.holes;
        while let [piece, rest @ ..] = slots {
            if index == 0 {
                return piece.is_empty();
            }
            index = index.saturating_sub(1);
            slots = rest;
        }
        true
    }

    /// The pieces, one more than the holes.
    fn pieces(&self) -> &[&'static str] {
        self.pieces
            .get(..self.holes.saturating_add(1))
            .unwrap_or(&[])
    }
}

/// Stores `piece` at `index` of `pieces`; `false` past their end.
const fn set(pieces: &mut [&'static str], index: usize, piece: &'static str) -> bool {
    let mut slots = pieces;
    let mut index = index;
    while let [slot, rest @ ..] = slots {
        if index == 0 {
            *slot = piece;
            return true;
        }
        index = index.saturating_sub(1);
        slots = rest;
    }
    false
}

/// The values of one line as a list, `(&first, (&second, ()))`, which
/// [`typed_line!`](crate::typed_line) builds: what the values take of the
/// note is known from their types when the line compiles.
pub trait LineValues {
    /// The values.
    const COUNT: usize;
    /// The note words they take, an error path's aside.
    const WORDS: usize;
    /// Whether an error's path comes before another value.
    const PATH_INSIDE: bool;
    /// Whether the last value is an error's path.
    const PATH_LAST: bool;

    /// Appends each piece and the value after it, then the last piece.
    fn push(&self, line: Line, pieces: &[&'static str]) -> Line;
}

impl LineValues for () {
    const COUNT: usize = 0;
    const WORDS: usize = 0;
    const PATH_INSIDE: bool = false;
    const PATH_LAST: bool = false;

    fn push(&self, line: Line, pieces: &[&'static str]) -> Line {
        match pieces.first() {
            Some(piece) => line.text(piece),
            None => line,
        }
    }
}

impl<V: LineValue + ?Sized, R: LineValues> LineValues for (&V, R) {
    const COUNT: usize = R::COUNT.saturating_add(1);
    const WORDS: usize = V::WORDS.saturating_add(R::WORDS);
    const PATH_INSIDE: bool = R::PATH_INSIDE || (V::PATH && R::COUNT > 0);
    const PATH_LAST: bool = if R::COUNT == 0 { V::PATH } else { R::PATH_LAST };

    fn push(&self, line: Line, pieces: &[&'static str]) -> Line {
        // The format has one piece more than the values, checked when the
        // line compiles.
        let (piece, rest) = pieces.split_first().map_or(("", &[][..]), |(p, r)| (*p, r));
        self.1.push(self.0.push(line.text(piece)), rest)
    }
}

impl Line {
    /// The line of `format` and its `values`. `HOLES` and `ENDS_OPEN` are
    /// the format's [`Format::holes`] and [`Format::ends_open`]; with the
    /// values' types they are checked at monomorphization: one value per
    /// hole, values within the note's words, and an error's path only last
    /// with no text after it. [`typed_line!`](crate::typed_line) passes them.
    pub fn format<V: LineValues, const HOLES: usize, const ENDS_OPEN: bool>(
        format: &Format,
        values: V,
    ) -> Self {
        const {
            if V::COUNT != HOLES {
                panic!("a line has one value for each hole of its format");
            }
            // The shape takes the first word.
            if V::WORDS >= MAX_WORDS {
                panic!("a line's values fit the note's words");
            }
            if V::PATH_INSIDE {
                panic!("an error's path is the last value of its line");
            }
            if V::PATH_LAST && !ENDS_OPEN {
                panic!("no text follows an error's path");
            }
        }
        values.push(Self::new(), format.pieces())
    }
}

/// One line of a format string and its values:
/// `typed_line!("ipv4 {}/{}", Ipv4(address), prefix)`. A hole is `{}` and
/// takes the next value, a [`LineValue`]; [`Hex`] and [`Ipv4`] choose a
/// value's text, and an error (a [`DiagnosticPath`]) is the last value.
/// The format, the values' count, their words and their place are checked
/// when the line compiles.
///
/// ```
/// let note = oer_diagnostics::typed_line!("ipv4 {}/{}", oer_diagnostics::Ipv4([10, 0, 0, 2]), 24_u8).note();
/// assert_eq!(note.names(), ["ipv4 ", "/"]);
/// ```
///
/// A brace that is not `{}`:
///
/// ```compile_fail,E0080
/// let _ = oer_diagnostics::typed_line!("connect-failed {:?}", 1_u8);
/// ```
///
/// A value for no hole:
///
/// ```compile_fail,E0080
/// let _ = oer_diagnostics::typed_line!("channel=", 6_u8);
/// ```
///
/// More words than the note holds:
///
/// ```compile_fail,E0080
/// let _ = oer_diagnostics::typed_line!("{}{}{}{}", 1_u64, 2_u64, 3_u64, 4_u64);
/// ```
///
/// An error's path before another value:
///
/// ```compile_fail,E0080
/// struct Failed;
/// impl oer_diagnostics::DiagnosticPath for Failed {
///     fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
///         path.name("Failed");
///     }
/// }
/// let _ = oer_diagnostics::typed_line!("tcp {} {}", Failed, 1_u8);
/// ```
///
/// Text after an error's path:
///
/// ```compile_fail,E0080
/// struct Failed;
/// impl oer_diagnostics::DiagnosticPath for Failed {
///     fn push_path(&self, path: &mut oer_diagnostics::DiagPath) {
///         path.name("Failed");
///     }
/// }
/// let _ = oer_diagnostics::typed_line!("tcp {} again", Failed);
/// ```
#[macro_export]
macro_rules! typed_line {
    ($format:literal $(, $value:expr)* $(,)?) => {{
        const FORMAT: $crate::line::Format = $crate::line::Format::parse($format);
        const {
            match FORMAT.error() {
                None => {}
                Some($crate::line::FormatError::Brace) => {
                    panic!("a line's format holds holes only as a bare pair of braces, without options or escapes")
                }
                Some($crate::line::FormatError::Segments) => {
                    panic!("a line has at most MAX_SEGMENTS pieces and values")
                }
                Some($crate::line::FormatError::Pieces) => {
                    panic!("a line's text pieces fit the note's names")
                }
                Some($crate::line::FormatError::Bytes) => {
                    panic!("a line's text pieces fit MAX_NAME and MAX_NAME_BYTES")
                }
            }
        };
        $crate::Line::format::<_, { FORMAT.holes() }, { FORMAT.ends_open() }>(
            &FORMAT,
            $crate::__line_values!($($value),*),
        )
    }};
}

/// The values of a line as the list [`LineValues`] walks.
#[doc(hidden)]
#[macro_export]
macro_rules! __line_values {
    () => {
        ()
    };
    ($first:expr $(, $rest:expr)*) => {
        (&$first, $crate::__line_values!($($rest),*))
    };
}

#[cfg(test)]
mod tests;
