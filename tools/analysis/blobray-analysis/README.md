# Blobray analyses

Blobray's analyses over the bounded CFG and value analysis of
[`oer-analysis-riscv-analysis`](../riscv-analysis/README.md): register-access
recognition, code-coverage closures and record
navigation. They have no filesystem authority.

`registers` recognizes load masks using the shared borrowed fact index.
One CFG engine runs two value domains with shared traversal, stack transfers,
ABI handling and callee-summary application. The read domain tracks selected
physical loads with exact/May dependency matrices and keeps entry sources for
argument summaries. The provenance domain uses one fixed tag per bit: unknown,
zero, one, an entry-register bit or a bit of a word identified by address and
load width. Equal word tags survive joins across distinct load instructions;
differing tags become unknown. Numeric address forms share one word identity;
unresolved addresses use their abstract-value identity. All loads, including
RAM outside the selected ranges, can supply stored provenance. Stack reloads
retain forwarded word/entry tags; other bits name the reloaded word. Known
values remain separate from word identity until stored projection.

`stored` is present for stores in the selected ranges, including selected indexed
addresses; plain unresolved stores carry none. At those stores, constants and
word/entry bit tags project to runs. Integer wiring and constant add/subtract
retain bits below the first possible carry or borrow; inversion and other
arithmetic become unknown. Numeric alternatives retain their shared constant
bits. A replacement mask covers stored bits not copied identically from the
same word and position; writes with no preserved bits carry no replacement mask.

State sets and queues use admitted working memory. Each domain has its own
`max(100000, 512 * instruction_count)` function budget: dependency operations
charge their work, while provenance charges instruction visits. Both observe
caller cancellation and deadlines throughout. On a provenance limit, only the
forward closure of pending transfers loses its proof; independent stores and
literal numeric facts remain available.

Application resolves call-argument callees against captured input and ROM
symbols. The read domain also computes argument consumption and returned bit
mappings, including inversion, through `riscv-model::calls::CallSummaries`.
Application computes the call graph without summaries, then analyzes functions
in callee-first SCC order with completed contracts. Owned argument CFG recipes
come from those final records and are released after solving. Recursive
components and unavailable or flow-incomplete callees retain May consumption.
Returned dependencies continue in caller results; summary work and storage use
the operation's existing limits.
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
that the expression observations require. The wrapper owns no tables or scans;
all structural lookups and memory-access visits use the model's index. Unknown
addresses remain explicit.
