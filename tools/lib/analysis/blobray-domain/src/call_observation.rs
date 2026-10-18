//! Explicit capture of physical call and opt-in tail-transfer boundaries.
use crate::*;
pub const MAX_CALL_CAPTURE_OVERRIDES: usize = 128;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallWordCount {
    pub target: u32,
    pub words: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallCapture {
    pub include_tail: bool,
    pub argument_words: u16,
    pub overrides: Vec<CallWordCount>,
}
impl CallCapture {
    pub fn validate(&self) -> Result<()> {
        let bad = || Error::new(ErrorCode::InvalidRequest, "invalid call capture policy");
        if self.argument_words as usize > MAX_EXECUTION_ARGUMENT_WORDS
            || self.overrides.len() > MAX_CALL_CAPTURE_OVERRIDES
        {
            return Err(bad());
        }
        for (i, o) in self.overrides.iter().enumerate() {
            if o.target & 1 != 0
                || o.target >= u32::MAX - 1
                || o.words as usize > MAX_EXECUTION_ARGUMENT_WORDS
                || self.overrides[..i].iter().any(|p| p.target == o.target)
            {
                return Err(bad());
            }
        }
        Ok(())
    }
    pub fn payload_bytes(&self) -> u64 {
        (self.overrides.len() * std::mem::size_of::<CallWordCount>()) as u64
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservedCallTarget {
    CapturedCode,
    CallModel,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WordAccess {
    UnknownStack,
    MisalignedStack,
    OutsidePrivateStack,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObservedWord {
    Known { value: u32 },
    Unknown,
    Unavailable { reason: WordAccess },
}
impl ObservedWord {
    pub fn value(self) -> Option<u32> {
        if let Self::Known { value } = self {
            Some(value)
        } else {
            None
        }
    }
}
