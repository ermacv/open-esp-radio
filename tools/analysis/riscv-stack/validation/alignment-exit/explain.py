#!/usr/bin/env python3
"""Check the shared ROM loop witness, both merge paths and unchanged call effects."""
import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROM_SHA256 = 'a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--before-records', type=Path, required=True)
    parser.add_argument('--before-effects', type=Path, required=True)
    parser.add_argument('--examples', type=Path, default=Path('target/tools/examples'))
    args = parser.parse_args()
    assert hashlib.sha256(args.rom.read_bytes()).hexdigest() == ROM_SHA256
    records_text = subprocess.check_output([
        str(args.examples / 'function-records'), str(args.rom), '2f80d260'], text=True)
    effects_text = subprocess.check_output([
        str(args.examples / 'stack-call-writes'), str(args.elf), str(args.rom),
        'chips/esp32s31/platform/rom-functions.toml', '5004db66', '-600'], text=True)
    rows = [row.split('\t') for row in records_text.splitlines()]
    effects = [row.split('\t') for row in effects_text.splitlines()]
    old = [row.split('\t') for row in args.before_records.read_text().splitlines()]
    old_effects = [row.split('\t') for row in args.before_effects.read_text().splitlines()]
    assert not [row for row in old if row[0] == 'aligned-value']
    assert [row for row in old if row[0] == 'graph'] == [row for row in rows if row[0] == 'graph']
    expressions = {int(row[1]): row[3] for row in rows if row[0] == 'expression'}
    for row in old:
        if row[0] == 'expression':
            assert expressions[int(row[1])] == row[3], row
    initial_ids = [id_ for id_, expression in expressions.items() if expression == 'Some(EntryRegister { register: 10 })']
    assert len(initial_ids) == 1
    initial = initial_ids[0]

    def fact(point, register):
        found = [row[1] for row in rows if row[0] == 'aligned-value'
                 and f'point: {point}, register: {register},' in row[1]]
        assert len(found) == 1, (point, register, found)
        return found[0]

    def edge(a, b):
        return f'Edge {{ from: {a}, target: {b} }}'

    checkpoint = 0x2f80d28c
    merged = fact(f'Entry {{ offset: {checkpoint} }}', 15)
    value = int(re.search(r', value: Expression \{ id: (\d+) \}', merged)[1])
    masked = re.fullmatch(r'Some\(Integer \{ op: And, left: Expression \{ id: (\d+) \}, right: Constant \{ value: 4294967292 \} \}\)', expressions[value])
    assert masked, expressions[value]
    added = int(masked[1])
    assert expressions[added] == f'Some(Integer {{ op: Add, left: Expression {{ id: {initial} }}, right: Constant {{ value: 3 }} }})'
    proof = re.search(r'proof: (AlignmentLoopProof \{[^}]+\})', merged)[1]
    assert f'owner: {0x2f80d260}' in merged
    assert 'origin: AllIncoming' in merged
    chain = []
    for point, register, origin in [
        (edge(0x2f80d288, 0x2f80d28a), 10, 'ZeroResidue'),
        (edge(0x2f80d28a, checkpoint), 15, 'Propagation'),
        (edge(0x2f80d2d0, 0x2f80d2d2), 15, 'LoopExit'),
        (edge(0x2f80d2d2, checkpoint), 15, 'Propagation'),
    ]:
        item = fact(point, register)
        assert f'initial: Expression {{ id: {initial} }}, value: Expression {{ id: {value} }}' in item
        assert f'proof: {proof}' in item and f'origin: {origin}' in item
        chain.append(item)
    incoming = [row[1] for row in rows if row[0] == 'graph'
                and f'target: Some({checkpoint})' in row[1]]
    assert len(incoming) == 2
    assert any(f'from: {0x2f80d28a}' in row for row in incoming)
    assert any(f'from: {0x2f80d2d2}' in row for row in incoming)
    instructions = {int(row[1], 16): row[2] for row in rows if row[0] == 'instruction'}
    for pc, instruction in {
        0x2f80d284: 'andi a5, a0, 3', 0x2f80d288: 'bne a5, zero, 76',
        0x2f80d28a: 'add a5, zero, a0', 0x2f80d2d4: 'add a5, zero, a0',
        0x2f80d2d6: 'j -10', 0x2f80d2cc: 'andi a4, a5, 3',
        0x2f80d2c4: 'addi a5, a5, 1', 0x2f80d2c8: 'sb a4, -1(a5)',
        0x2f80d2d0: 'bne a4, zero, -16', 0x2f80d2d2: 'j -70',
    }.items():
        assert instructions[pc] == instruction
    assert [row[2:] for row in rows if row[:2] == ['register-write', '0x2f80d28a']] == [['15', f'Expression {{ id: {initial} }}']]
    assert not [row for row in rows if row[:2] == ['register-write', '0x2f80d2d0']]
    # The actual architectural writes stay separate from derived control facts.
    assert [row for row in rows if row[:2] == ['register-write', '0x2f80d2d2']] == [['register-write', '0x2f80d2d2', '0', 'Constant { value: 0 }']]
    for kind in ('rom-write', 'rom-barrier', 'call-write', 'call-barrier', 'preserves', 'inputs', 'target'):
        assert [row for row in effects if row[0] == kind] == [row for row in old_effects if row[0] == kind], kind
    writes = [row for row in effects if row[0] == 'rom-write' and row[2] == '0x2f80d260']
    barriers = [row for row in effects if row[0] == 'rom-barrier' and row[2] == '0x2f80d260']
    assert len(writes) == 14 and len({row[1] for row in writes}) == 12
    assert len(barriers) == 12 and sum('kind: UnknownStore' in row[3] for row in barriers) == 11
    finite = [row[3] for row in writes if row[1] == '0x2f80d2c8']
    assert len(finite) == 3
    for offset, row in enumerate(finite):
        assert f'address: Base {{ base: Argument(0), offset: {offset} }}' in row
        assert f'AlignmentLoop({proof})' in row
        assert 'occurrence: May' in row
    assert ['preserves', '0x5004db66', 'slot=-600', 'false'] in effects
    assert any(row[:2] == ['target', '0x5004db66'] and '796918980' in row[2] for row in effects)
    result = {
        'baseline': '97ffda65',
        'rom_sha256': ROM_SHA256,
        'elf_sha256': hashlib.sha256(args.elf.read_bytes()).hexdigest(),
        'function': '0x2f80d260', 'checkpoint': f'{checkpoint:#010x}',
        'initial_expression': {'id': initial, 'definition': expressions[initial]},
        'sum_expression': {'id': added, 'definition': expressions[added]},
        'aligned_expression': {'id': value, 'definition': expressions[value]},
        'checkpoint_fact': merged,
        'incoming_edges': incoming,
        'chain': chain,
        'shared_loop_proof': proof,
        'representation': 'AlignedValue at Entry/Edge; ordinary Add/And DAG; owner and initial retained; no synthetic architectural Value',
        'postconditions': ['a5 = (argument0.wrapping_add(3)) & !3',
                           'a5 & 3 = 0', 'a5 = argument0 + d (mod 2^32), d in 0..=3'],
        'architectural_graph_and_original_expressions_unchanged': True,
        'rom_and_call_effects_identical_to_main': True,
        'store_instructions': 12, 'store_effects': 14, 'remaining_unknown_store_sites': 11,
        'fresh_call': '0x5004db66', 'call_preserves_queried_word': False,
        'caller_note': 'Main #178 changed the firmware layout. The old call PC 0x5004dd24 is no longer a transfer. The fresh memcpy call has unknown a0; -600 is a word-exclusion query, not a newly identified spill.',
    }
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'rom-records.tsv').write_text(records_text)
    (args.output / 'effects.tsv').write_text(effects_text)
    (args.output / 'proof.json').write_text(json.dumps(result, indent=2) + '\n')
    print('Both ROM merge paths prove align-up; the shared 3-byte footprint and all remaining call barriers are unchanged.')


if __name__ == '__main__':
    main()
