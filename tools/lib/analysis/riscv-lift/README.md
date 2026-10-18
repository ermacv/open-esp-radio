# RV32 function decoding, relocation interpretation and lifting

`oer-riscv-lift` owns the `FunctionDecoder` and `FunctionSemantics`
implementations of [`oer-riscv-model`](../model/README.md) and RISC-V relocation
interpretation. It receives bytes and structural facts, never a
archive path or loader capability. Unsupported encodings remain
explicit gaps. Relocation types are classified by the one RV32 relocation
table of [`oer-elf`](../../elf/README.md) (`rv32::kind`), never a local list.

`listing::normalize` writes a linked function as placement-independent lines:
it decodes each instruction and replaces every address the code forms
(branch and jump targets, `lui`/`auipc` pairs and what completes them through
registers, address words in code) by the location the caller's `Placement`
names. `cargo fw compare` compares these listings instead of
disassembler text.

Decoding belongs to [`oer-riscv-decode`](../decode/README.md),
selected with every extension it decodes: ESP-IDF builds the ESP32-S31 for
`rv32imafc_zba_zbb_zbs_zcb_zcmp_zcmt` with Zicsr, and only the Zcmt table
jumps stay undecoded. Integer and Zcb memory forms lift to single operations;
`IntegerOp::evaluate` in the model is the one concrete definition that
analysis and execution share. The Zcmp forms move several registers and lift
to `Unsupported`, so abstract analysis keeps a gap for them while concrete
execution runs them: pushes store the listed registers from `s11` down to `ra`
below `sp`, pops load them back, and the returning forms return through `ra`,
as the decoded flow states.

The FP register file is not modeled: single-precision loads and stores lift to
`FloatLoad`/`FloatStore` accesses through their integer base, a form with an
integer destination (`fmv.x.w`, `fclass.s`, comparisons, `fcvt.w[u].s`) lifts
to `Opaque`, and every other form to `None`. A Zicsr access writes only its
`rd` among the integer registers and lifts to `Opaque` of it.

A `jalr` through `ra` or `t0` at offset zero that links neither decodes as
`InstructionFlow::Return` (the ISA's pop hint), as do the returning Zcmp pops;
every other `jalr` is `Indirect`, so no consumer recognizes returns by
register.

The decoder and semantic identities (`policy-7` and `values-10`, each over
rv-asm 0.2.1) cover every behavior below; any change to decoding or lifting
changes its identity. `decode_instruction` is the decoding Blobray's executor
shares.

Lifting returns bounded typed operations over RV32 registers. Loads, stores and
atomics describe effects without reading memory. Compressed instructions use the
shared normalized operands, including the decoder's signed C.ANDI immediate.
The backend declares relocation roles;
analysis validates the flowing address relationship and owns abstract states.

The semantic identity includes typed fence mode/predecessor/successor
sets. Analysis consumes the typed fence record; it never parses
instruction display strings.
