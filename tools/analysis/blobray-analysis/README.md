# Blobray analyses

Blobray's analyses over the bounded CFG and value analysis of
[`oer-analysis-riscv-analysis`](../riscv-analysis/README.md): register-access
recognition, code-coverage closures and record
navigation. They have no filesystem authority.

`registers` recognizes load masks and evaluates stored bit provenance using
the shared borrowed fact index. A read-modify-write's replacement mask covers
the stored bits not copied identically from the same word and position;
writes with no preserved bits carry no replacement mask. It reports
bit-selection observations with their original physical load width;
application owns address scope.
Its independent read-use pass propagates load-site dependencies over the
instruction CFG to a fixed point, with union at joins, explicit limits and
byte-level stack transfers. Values also carry known-zero and known-one masks;
joins retain only bits known equally on every path. Neutral bitwise operands
pass dependencies exactly; inverted bits remain uses, never preservation.
Interned state sets and the queue have working
memory admission; each function's pass also has a
`max(100000, 512 * instruction_count)` work-unit cap. Application resolves
call-argument callees against captured input and ROM symbols. The same engine
also seeds entry registers for argument summaries: consumed bits and returned
bit mappings, including inversion, through the shared
`riscv-model::calls::CallSummaries` port. Application computes the call graph
without summaries, then analyzes functions in callee-first SCC order with
completed callee contracts. Compact argument CFG recipes come from final
records in this second analysis and are released after solving.
Recursive components and unavailable or flow-incomplete callees retain May
argument consumption. Returned dependencies continue in the caller's results;
summary work and storage use the operation's existing limits.
It never infers register geometry, hardware field meaning or safe RMW semantics.

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
