//! JSON documents printed by the `blobray` CLI with `--format json`.
//!
//! The renderer emits these shapes and typed clients decode them; payloads are
//! the application/domain records themselves, not copies of their schemas.
use oer_analysis_blobray_domain::{
    CheckVerdict, ForbiddenTargetRange, LibraryFunction, RegisterAccess, RegisterAccessSummary,
    TargetAuditRecord, TargetAuditSummary,
};
use oer_analysis_riscv_model::{
    ArtifactId, CallAbi, Error, FunctionCoverage, FunctionRecord, ReadOnlyWindow, SemanticSummary,
};
use serde::{Deserialize, Serialize};

/// Schema of the `audit-targets` document.
pub const TARGET_AUDIT_SCHEMA: u32 = 1;

/// Every finding of one final-image audit and its verdict: `pass` only
/// without a forbidden target or a coverage gap.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetAuditDocument {
    pub schema: u32,
    /// SHA-256 of the audited image.
    pub artifact: ArtifactId,
    pub decoder: String,
    pub semantics: String,
    pub ranges: Vec<ForbiddenTargetRange>,
    pub records: Vec<TargetAuditRecord>,
    pub summary: TargetAuditSummary,
    pub verdict: CheckVerdict,
}

/// Schema of the `register-accesses` document
/// (`{"schema":9,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"records":[...],"groups":[...],"summary":{...}}`;
/// `groups` only with `--group-by`).
pub const REGISTER_ACCESSES_SCHEMA: u32 = 9;

/// One analyzed input of a `register-accesses` document, by position.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessInput {
    pub role: String,
    pub sha256: ArtifactId,
}

/// Every register access of the analyzed inputs; see `REGISTER_ACCESSES_SCHEMA`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    /// The records the request's filter selected: its observations, every
    /// blocked function it names and every gap.
    pub records: Vec<RegisterAccess>,
    /// The selected observations grouped as `--group-by` requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<crate::access_groups::AccessGroup>>,
    /// The whole analysis, independent of the filter.
    pub summary: RegisterAccessSummary,
}

/// Schema of the `function-records` document
/// (`{"schema":9,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"functions":[...],"missing":[...]}`).
pub const FUNCTION_RECORDS_SCHEMA: u32 = 9;
pub const FIELD_ACCESSES_SCHEMA: u32 = 5;
pub const CALLERS_SCHEMA: u32 = 4;

/// Every memory access of the analyzed functions that lands on one field
/// offset, as a field of its root ([`crate::field`]), and the functions no
/// analysis could read.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccessesDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    /// The field's displacement from the last loaded pointer or the root.
    pub offset: i64,
    /// The access width the request selects, if any.
    pub width: Option<u8>,
    /// The functions with at least one such access, in input, object and
    /// symbol order.
    pub functions: Vec<FieldAccessFunction>,
    /// Functions that cannot be analyzed: their accesses are unknown.
    pub blocked: Vec<LibraryFunction>,
    /// Analyzed functions whose coverage or value semantics is incomplete:
    /// some of their accesses may be missing above.
    pub partial: u64,
    /// `partial` by cause.
    pub partial_causes: oer_analysis_blobray_domain::PartialCauses,
    /// Code no function covers, whose accesses are unknown too.
    pub gaps: u64,
    /// Accesses of the analyzed functions whose address is not known at
    /// all, so no field selection sees them.
    pub unknown_addresses: u64,
}

/// One function's accesses of the requested field.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccessFunction {
    pub function: LibraryFunction,
    pub accesses: Vec<crate::field::FieldAccess>,
}

/// The complete analysis of every function the request names, in input,
/// object and symbol order, and the names no input defines.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRecordsDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    pub functions: Vec<NamedFunction>,
    /// Requested names that no analyzed or blocked function carries.
    pub missing: Vec<String>,
}

/// One function a `function-records` request names.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NamedFunction {
    /// Its records: every fact, expression and value the analysis derived.
    Analyzed {
        function: LibraryFunction,
        /// Coverage and value semantics are both complete.
        complete: bool,
        coverage: FunctionCoverage,
        semantics: SemanticSummary,
        records: Vec<FunctionRecord>,
        /// Ordered tables or complete target sets proven by the build's facts.
        /// Each payload explicitly distinguishes index mappings from sets.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jump_tables: Vec<oer_analysis_riscv_model::JumpDispatch>,
    },
    /// It cannot be analyzed; its behavior is unknown.
    Blocked {
        function: LibraryFunction,
        error: Error,
    },
}

/// Call/branch references and candidate pointer storage for requested symbols.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallersDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    /// The requested symbol names.
    pub symbols: Vec<String>,
    /// The referencing functions, in input, object and symbol order.
    pub callers: Vec<Caller>,
    /// Candidates for indirect calls, not proven calls.
    pub stored: Vec<crate::references::Stored>,
    pub scan: crate::references::ScanSummary,
    pub functions: u64,
    pub partial: u64,
    pub partial_causes: oer_analysis_blobray_domain::PartialCauses,
    /// Functions that cannot be analyzed: their references are unknown.
    pub blocked: Vec<LibraryFunction>,
    /// Code no function covers.
    pub gaps: u64,
}

/// One function's references to the requested symbols.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub function: LibraryFunction,
    pub references: Vec<crate::references::Reference>,
}

/// Schema of the `call-arguments` document
/// (`{"schema":5,"inputs":[...],"abi":...,"call_returns":...,"read_only":[...],"symbols":[...],"callers":[...],"blocked":[...],"partial":N,"partial_causes":{...},"gaps":N}`).
pub const CALL_ARGUMENTS_SCHEMA: u32 = 5;

/// The argument registers at every call site of the named functions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallArgumentsDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    /// The requested function names.
    pub symbols: Vec<String>,
    /// The calling functions with their call sites, in input, object and
    /// symbol order.
    pub callers: Vec<CallingFunction>,
    /// Functions that cannot be analyzed: their calls are unknown.
    pub blocked: Vec<LibraryFunction>,
    /// Analyzed functions whose coverage or value semantics are incomplete:
    /// they can hold further call sites or less precise values.
    pub partial: u64,
    /// `partial` by cause.
    pub partial_causes: oer_analysis_blobray_domain::PartialCauses,
    /// Code no function covers.
    pub gaps: u64,
}

/// One function with its call sites of the named functions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallingFunction {
    pub function: LibraryFunction,
    pub sites: Vec<crate::call_arguments::CallSite>,
}

/// Schema of the `mmio-diff` document.
pub const MMIO_DIFF_SCHEMA: u32 = 4;

/// The MMIO difference of two builds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioDiffDocument {
    pub schema: u32,
    pub old: Vec<RegisterAccessInput>,
    pub new: Vec<RegisterAccessInput>,
    pub abi: Option<CallAbi>,
    /// Whether calls took the constants their callees return.
    pub call_returns: bool,
    /// The windows the analysis took as read-only.
    pub read_only: Vec<ReadOnlyWindow>,
    #[serde(flatten)]
    pub diff: oer_analysis_blobray_domain::MmioDiff,
}

/// Schema of the `symbols` document (`{"schema":1,"inputs":[...],"symbols":[...]}`).
pub const SYMBOLS_SCHEMA: u32 = 1;

/// The defined function and data symbols of the inputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolsDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    pub symbols: Vec<crate::symbols::SymbolRow>,
}

/// Schema of the `strings` document (`{"schema":1,"inputs":[...],"strings":[...]}`).
pub const STRINGS_SCHEMA: u32 = 1;

/// The NUL-terminated text of the inputs' read-only data sections.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringsDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    pub strings: Vec<StringRow>,
}

/// One string, by input, archive member, section and section offset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringRow {
    pub input: u64,
    pub member: Option<String>,
    pub section: Option<String>,
    pub offset: u64,
    /// Escaped text: printable ASCII with `\\n`, `\\t`, `\\r`, `\\\\`, `\\"` and `\\xNN`.
    pub text: String,
}
