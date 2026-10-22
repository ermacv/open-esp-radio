//! Public-header ABI contracts. Parsing and validation belong to application;
//! neither the CLI nor register checking interprets the file independently.
use crate::*;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureResult {
    Void,
    U32,
    U64,
}
impl SignatureResult {
    fn words(self) -> u8 {
        match self {
            Self::Void => 0,
            Self::U32 => 1,
            Self::U64 => 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionSignature {
    pub name: String,
    /// Occupied RV32 a-register words, rather than C parameter count.
    pub arguments: u8,
    pub result: SignatureResult,
    pub source: String,
}
impl FunctionSignature {
    pub(crate) fn summary(&self) -> CallSummary {
        let mut summary = CallSummary {
            contract: Some(CallContract {
                arguments: self.arguments,
                results: self.result.words(),
            }),
            contract_only: true,
            ..CallSummary::default()
        };
        for argument in summary
            .arguments
            .consumed
            .iter_mut()
            .take(usize::from(self.arguments))
        {
            argument.may = u32::MAX;
        }
        summary
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Signatures(Vec<FunctionSignature>);
impl Signatures {
    pub fn parse(text: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            function: Vec<FunctionSignature>,
        }
        let mut file: File = toml::from_str(text)
            .map_err(|e| Error::new(ErrorCode::InvalidRequest, format!("signatures: {e}")))?;
        file.function.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        for (index, signature) in file.function.iter().enumerate() {
            if signature.arguments > 8 || signature.name.is_empty() || signature.source.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "invalid signature {}: arguments must be <= 8; name and source must be nonempty",
                        signature.name
                    ),
                ));
            }
            if index != 0
                && file
                    .function
                    .get(index - 1)
                    .is_some_and(|s| s.name == signature.name)
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!("duplicate signature {}", signature.name),
                ));
            }
        }
        Ok(Self(file.function))
    }
    pub fn get(&self, name: &[u8]) -> Option<&FunctionSignature> {
        self.0
            .binary_search_by(|s| s.name.as_bytes().cmp(name))
            .ok()
            .and_then(|i| self.0.get(i))
    }
    pub fn functions(&self) -> &[FunctionSignature] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_results_arity_and_duplicate_names() {
        let record = |result, arguments| {
            format!(
                "[[function]]\nname='f'\narguments={arguments}\nresult='{result}'\nsource='header.h:1'\n"
            )
        };
        for text in [
            record("float", 1),
            record("void", 9),
            record("u32", 1).repeat(2),
        ] {
            assert_eq!(
                Signatures::parse(&text).unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
        for (result, words) in [("void", 0), ("u32", 1), ("u64", 2)] {
            let parsed = Signatures::parse(&record(result, 3)).unwrap();
            let summary = parsed.get(b"f").unwrap().summary();
            assert_eq!(summary.contract.unwrap().results, words);
            assert_eq!(summary.arguments.consumed[2].may, u32::MAX);
            assert_eq!(summary.arguments.consumed[3].may, 0);
        }
    }
}
