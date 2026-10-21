//! The inventory's JSON boundary. Read classes follow the ReadUses contract
//! in #266/#277; the checker does not compute dataflow or infer sinks.
use crate::Result;
use oer_analysis_blobray_domain::{RegisterMask, StoredBits};
use serde::{Deserialize, Serialize};

pub(super) struct Document {
    pub schema: u32,
    pub corpus: Corpus,
    pub records: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Input {
    pub role: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Summary {
    pub functions: u64,
    pub partial_functions: u64,
    pub partial_causes: serde_json::Value,
    pub blocked_functions: u64,
    pub gaps: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
    pub indexed_addresses: u64,
    pub ranges: Vec<serde_json::Value>,
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Corpus {
    pub inputs: Vec<Input>,
    pub abi: Option<String>,
    pub call_returns: bool,
    pub read_only: Vec<serde_json::Value>,
    pub summary: Summary,
}

impl Document {
    pub fn parse(json: &str, inputs: &[String]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Header {
            schema: u32,
        }
        let header: Header = serde_json::from_str(json)?;
        if header.schema != 9 {
            return Err(format!(
                "vendor-use checker requires register-accesses schema 9; got {}",
                header.schema
            )
            .into());
        }
        #[derive(Deserialize)]
        struct Body {
            #[serde(flatten)]
            corpus: Corpus,
            records: Vec<serde_json::Value>,
        }
        let body: Body = serde_json::from_str(json)?;
        if !body.corpus.inputs.iter().map(|i| &i.role).eq(inputs) {
            return Err("register-accesses inputs differ from the requested corpus".into());
        }
        let (mut observations, mut blocked, mut gaps, mut unresolved, mut indexed) =
            (0, 0, 0, 0, 0);
        for record in &body.records {
            match record.get("kind").and_then(|kind| kind.as_str()) {
                Some("observation") => {
                    observations += 1;
                    unresolved +=
                        u64::from(record.get("address").is_none_or(serde_json::Value::is_null));
                    indexed +=
                        u64::from(record.get("indexed").is_some_and(|value| !value.is_null()));
                }
                Some("blocked") => blocked += 1,
                Some("gap") => gaps += 1,
                _ => return Err("unknown register-accesses record kind".into()),
            }
        }
        let summary = &body.corpus.summary;
        if (observations, blocked, gaps, unresolved, indexed)
            != (
                summary.observations,
                summary.blocked_functions,
                summary.gaps,
                summary.unresolved_addresses,
                summary.indexed_addresses,
            )
        {
            return Err("register-accesses records do not match coverage summary (filtered or incomplete corpus)".into());
        }
        Ok(Self {
            schema: header.schema,
            corpus: body.corpus,
            records: body.records,
        })
    }
}

#[derive(Deserialize)]
pub(super) struct LibraryFunction {
    input: usize,
    name: Option<String>,
    pub symbol: serde_json::Value,
}

impl LibraryFunction {
    pub fn resolve(&self, inputs: &[String]) -> Result<super::Function> {
        let input = inputs
            .get(self.input)
            .ok_or("MMIO function of an unknown input")?;
        let name = self
            .name
            .clone()
            .ok_or("MMIO observation of an unnamed function")?;
        Ok((input.clone(), name))
    }
}

#[derive(Deserialize)]
pub(super) struct Fact {
    pub kind: String,
    pub access: Option<String>,
    pub width: Option<u8>,
    pub offset: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct Observation {
    pub function: LibraryFunction,
    pub record: u64,
    pub fact: Fact,
    pub address: Option<u32>,
    pub indexed: Option<super::super::Indexed>,
    pub mask: Option<RegisterMask>,
    pub stored: Option<Vec<StoredBits>>,
    pub reads: Option<ReadUses>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadUses {
    pub uses: Vec<ReadUse>,
    pub unknown: Vec<ReadLimit>,
    pub preserved: u32,
    pub unused: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadUse {
    pub bits: u32,
    pub sink: ReadSink,
    pub precision: ReadPrecision,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ReadSink {
    Branch {
        site: u64,
    },
    Dispatch {
        site: u64,
    },
    IndirectTarget {
        site: u64,
    },
    Address {
        site: u64,
    },
    Return {
        register: u8,
    },
    Store {
        site: u64,
        address: Option<u32>,
        same_word: bool,
    },
    CallArgument {
        site: u64,
        register: u8,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ReadPrecision {
    Exact,
    May,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadLimit {
    pub bits: u32,
    pub limit: ReadLimitKind,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ReadLimitKind {
    PartialFunction,
    StackSpill,
    UnsupportedOperation,
    WorkLimit,
}
