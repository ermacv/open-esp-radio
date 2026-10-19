# Blobray analyses

Blobray's analyses over the bounded CFG and value analysis of
[`oer-analysis-riscv-analysis`](../riscv-analysis/README.md): register-access
recognition, final-image target audits, code-coverage closures and record
navigation. They have no filesystem authority.

`registers` recognizes bounded load/mask/store expression shapes using
the shared borrowed fact index. It reports bit-selection observations with their
original physical load width; application owns address scope.
It never infers register geometry, hardware field meaning or safe RMW semantics.

The `audit` module scans all supplied executable ranges linearly for direct and
locally resolved transfers. It shares the ISA port and `fold_integer` with value
analysis; it does not obtain filesystem access or claim dynamic-target completeness.

`closure::code_closure` walks the code statically reachable from root entries
by recursive descent: direct branches, jumps and calls, `auipc`/`lui` plus
`jalr` pairs with a known target, and the caller-supplied observed targets of
executed indirect transfers. A jump to another function's start is a tail
transfer; declared boundaries stop the walk. Each function reports its blocks,
branch directions, callees, modeled boundaries and unresolved or
observation-followed transfers, which application's code coverage compares
with executed instructions. Decoding shares the working capacity and run
control; the function count is bounded.

`navigation::Facts` requires canonical records through the shared
[`FunctionIndex`](../riscv-model/src/navigation.rs), preserving the earlier-ID DAG
that the expression evaluators require. The wrapper owns no tables or scans;
all structural lookups and memory-access visits use the model's index. Unknown
addresses remain explicit.
