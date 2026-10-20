"""Check the current doorbell chain using the existing diagnostic fact streams.

This is a fixture validator, not a resolver. PCs/names select and assert the
fresh-main fixture; unknown memory contents never become a pointer identity.
"""
import argparse
import ast
from collections import deque
import hashlib
import json
from pathlib import Path
import re
import subprocess

from explain import contexts, diagnostic

ROM_SHA256 = 'a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87'
EXPRESSION = re.compile(r'Expression \{ id: (\d+) \}')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def one(items):
    items = list(items)
    assert len(items) == 1, items
    return items[0]


def items(text):
    """Split the top level of an existing Rust Debug sequence."""
    result, start, depth = [], 0, 0
    for index, char in enumerate(text):
        if char in '[{(': depth += 1
        elif char in ']})': depth -= 1
        elif char == ',' and depth == 0:
            result.append(text[start:index].strip())
            start = index + 1
    result.append(text[start:].strip())
    return result


class Records:
    def __init__(self, path):
        self.rows = [line.split('\t') for line in path.read_text().splitlines()]
        self.owner = one(r[1:] for r in self.rows if r[0] == 'function')
        self.expressions = {int(r[1]): r[2:] for r in self.rows if r[0] == 'expression'}

    def at(self, kind, pc):
        return [r[2:] for r in self.rows if r[:2] == [kind, f'{pc:#010x}']]

    def dependencies(self, text):
        selected, pending = {}, list(map(int, EXPRESSION.findall(text)))
        while pending:
            ident = pending.pop()
            if ident in selected: continue
            fact = self.expressions[ident]
            selected[ident] = {'pc': fact[0], 'expression': fact[1]}
            pending.extend(map(int, EXPRESSION.findall(fact[1])))
        return selected

    def snapshot(self, pc):
        facts = {kind: self.at(kind, pc) for kind in
                 ('instruction', 'register-write', 'memory', 'transfer', 'inputs',
                  'write-event', 'stack-reload', 'write-barrier') if self.at(kind, pc)}
        return {'pc': f'{pc:#010x}', 'owner': self.owner,
                'inline_context': [ast.literal_eval(r[0]) for r in self.at('inline-context', pc)],
                'facts': facts, 'expressions': self.dependencies(str(facts))}

    def instruction(self, pc, expected):
        assert self.at('instruction', pc) == [[expected]], (hex(pc), expected)

    def memory(self, pc, kind, address, value):
        assert self.at('memory', pc) == [[kind, '4', address, value]], self.snapshot(pc)

    def inputs(self, pc):
        text = one(self.at('inputs', pc))[0]
        assert text.startswith('Some([') and text.endswith('])')
        result = items(text[6:-2])
        assert len(result) == 32
        return result

    def add_entry(self, value, register, delta):
        ident = int(one(EXPRESSION.fullmatch(value).groups()))
        text = self.expressions[ident][1]
        match = re.fullmatch(r'Some\(Integer \{ op: Add, left: Expression \{ id: (\d+) \}, right: Constant \{ value: (\d+) \} \}\)', text)
        assert match and int(match[2]) == delta, text
        assert self.expressions[int(match[1])][1] == f'Some(EntryRegister {{ register: {register} }})'

    def reached(self, *pcs):
        # Independently check reachability over emitted edges, never decode or
        # manufacture edges. This checks the fixture, not branch feasibility.
        edges = []
        for row in self.rows:
            if row[0] == 'graph' and (m := re.fullmatch(r'Edge \{ from: (\d+), target: Some\((\d+)\), relation: (\w+), external: false \}', row[1])):
                edges.append((int(m[1]), int(m[2])))
        reached, queue = {int(self.owner[0], 16)}, deque([int(self.owner[0], 16)])
        while queue:
            source = queue.popleft()
            for src, target in edges:
                if src == source and target not in reached:
                    reached.add(target)
                    queue.append(target)
        assert set(pcs) <= reached, [hex(pc) for pc in set(pcs) - reached]


def validate(args):
    args.output.mkdir(parents=True, exist_ok=True)
    assert sha(args.rom) == ROM_SHA256
    summaries = Path('chips/esp32s31/platform/rom-functions.toml')
    chip = Path('chips/esp32s31/platform/chip.toml')
    report = args.elf.parent / 'interrupt-stack.txt'
    streams = []

    def capture(name, tool, *arguments):
        path = args.output / name
        with path.open('w') as out:
            subprocess.run([str(args.examples / tool), *map(str, arguments)], stdout=out, check=True)
        streams.append(path)
        return path

    def records(name, pc, *flags):
        return Records(capture(name + '.tsv', 'function-records', args.elf, f'{pc:x}',
                               '--inline-context', '--writes', *flags))

    audit_path = capture('callbacks.tsv', 'callback-audit', args.elf, args.rom, summaries)
    audit = diagnostic(audit_path)
    call = int(one(pc for pc, row in audit['unresolved'].items()
                   if ast.literal_eval(row[3])[-1:] == ['oer_firmware_apps_w1::irq::doorbell']
                   and 'SignalPend>::pend' in row[3]), 16)
    assert call == 0x2f00371e
    irq_context = contexts(report)[f'{call:#010x}']
    assert irq_context and all(c['hart'] == 0 and c['level'] == 3 for c in irq_context)
    handler = records('doorbell', call, '--image-facts', '--interrupt-contract', chip)
    root = records('irq-root', 0x2f00428e)
    constructor = records('rx-poll', 0x5006bbca, '--symbolic-spills')
    run = records('run', 0x5004db66, '--symbolic-spills')
    pend = records('pend-initializer-target', 0x2f003222)

    symbols = {r[4]: (int(r[1], 16), int(r[2])) for r in handler.rows if r[0] == 'symbol'}
    signal, signal_size = symbols['oer_firmware_apps_w1::irq::MAC_SIGNAL']
    cell, cell_size = symbols['oer_firmware_apps_w1::irq::RX_LINE']
    arena, _ = symbols['oer_firmware_apps_w1::irq::RX_ARENA']
    assert (signal, signal_size, cell, cell_size, arena) == (0x2f007004, 36, 0x2f0174c0, 8, 0x2f017540)
    entry = one(r for r in handler.rows if r[0] == 'irq-entry' and r[1] == '2')
    assert entry[2:] == ['0x500a5200', 'TableEntry { source: 68, level: 3, core: 0, handler: Some(788546190) }']
    root.instruction(0x2f004292, 'jalr zero, 846(t1)')
    assert root.at('transfer', 0x2f004292) == [[f'Some(ImageAddress {{ address: {int(handler.owner[0], 16)} }})']]
    assert any(r[:3] == ['image-word', '0x500a5200', '0x2f00428e'] for r in handler.rows)

    # Follow expression identities actually consumed by the indirect call.
    load_specs = [
        (0x2f00369e, 'lw s1, 1216(a0)', f'Constant {{ value: {cell} }}', 47),
        (0x2f0036fe, 'lw a0, 0(s1)', 'Expression { id: 47 }', 61),
        (0x2f003706, 'lw a0, 0(a0)', 'Expression { id: 61 }', 64),
        (0x2f003708, 'lw a0, 0(a0)', 'Expression { id: 64 }', 65),
        (0x2f00371c, 'lw a1, 0(a0)', 'Expression { id: 65 }', 68),
    ]
    for pc, instruction, address, ident in load_specs:
        handler.instruction(pc, instruction)
        handler.memory(pc, 'Load', address, 'None')
        assert handler.expressions[ident] == [f'{pc:#010x}', f'Some(Load {{ address: {address}, width: 4, signed: false }})']
    handler.instruction(call, 'jalr ra, 0(a1)')
    assert handler.inputs(call)[10:12] == ['Expression { id: 65 }', 'Expression { id: 68 }']
    assert handler.at('transfer', call) == [['Some(Unknown)']]
    producer = audit['unresolved'][f'{call:#010x}'][-1]
    assert 'exclusive_producer: true' in producer and f'site: {0x2f00371c}' in producer
    field = one(audit['loads'][f'{call:#010x}'])
    assert field[2] == '0x2f00371c' and 'Some(CallbackLoadField' in field[3]
    assert 'MemoryWrite' in field[4] and str(0x2f003712) in field[4]
    candidates = audit['sites'][('typed', f'{call:#010x}')]
    assert '0x2f003222' in candidates
    handler.reached(*(pc for pc, *_ in load_specs), call)

    initializer = one(r for r in handler.rows if r[0] == 'image-word' and r[1] == f'{signal:#010x}')
    assert initializer[2:] == ['0x2f003222', 'read-only=None']
    static_use = one(r for r in audit_path.read_text().splitlines() if r.startswith('use\t0x2f003222\t') and '\t0x2f007004\tWord\t0\ttyped:' in r)
    use_index = static_use.split('\t')[2]
    address_use = one(r for r in handler.rows if r[:2] == ['address-use', use_index])
    assert f'at: {signal},' in address_use[2] and f'value: Resolved({0x2f003222})' in address_use[2]

    # Exact construction stores are proved computations, not cross-call memory
    # postconditions. The initializer is in writable data as well.
    constructor.memory(0x5006c590, 'Store', f'Constant {{ value: {arena} }}', f'Some(Constant {{ value: {signal} }})')
    constructor.memory(0x5006c592, 'Store', f'Constant {{ value: {arena + 4} }}', f'Some(Constant {{ value: {arena} }})')
    for pc, delta, value in [(0x5006c5ee, 0x580, arena + 4),
                             (0x5006c602, 0x590, arena + 0x2c0),
                             (0x5006c606, 0x594, arena + 4),
                             (0x5006c6f4, 0x15c, cell)]:
        access = one(constructor.at('memory', pc))
        assert access[:2] == ['Store', '4'] and access[3] == f'Some(Constant {{ value: {value} }})'
        constructor.add_entry(access[2], 10, delta)

    copies = []
    for pc, length in [(0x5006c6ae, 68), (0x5006c6c0, 284), (0x5006bcc4, 640)]:
        inputs = constructor.inputs(pc)
        assert inputs[12] == f'Constant {{ value: {length} }}'
        assert constructor.at('transfer', pc) == [[f'Some(ImageAddress {{ address: {0x2f8004c4} }})']]
        copies.append(constructor.snapshot(pc))
    constructor.add_entry(constructor.inputs(0x5006c6ae)[11], 10, 0x554)
    assert constructor.inputs(0x5006c6ae)[10] == 'EntryStack { offset: -280 }'
    constructor.add_entry(constructor.inputs(0x5006c6c0)[10], 10, 0x38)
    assert constructor.inputs(0x5006c6c0)[11] == 'EntryStack { offset: -496 }'
    constructor.add_entry(constructor.inputs(0x5006bcc4)[10], 10, 0x2b8)
    constructor.add_entry(constructor.inputs(0x5006bcc4)[11], 10, 0x38)
    first_copy = capture('first-constructor-copy.tsv', 'stack-call-writes', args.elf, args.rom,
                         summaries, '5006c6ae', '-236', '--word-offset', '44')
    first_copy_rows = [line.split('\t') for line in first_copy.read_text().splitlines()]
    assert ['context-preserves', 'false'] in first_copy_rows
    assert len([r for r in first_copy_rows if r[0] == 'context-call-write']) == 14
    word_rows = [r for r in first_copy_rows if r[0] == 'post-word']
    assert {r[1] for r in word_rows} == {'0x2f80d260', '0x2f8004c4'}
    for row in word_rows:
        for required in (f'function: {0x2f80d260}', 'destination: Base { base: Argument(0), offset: 44 }',
                         'source: Base { base: Argument(1), offset: 44 }',
                         f'load: {0x2f80d332}, load_order: 51', f'store: {0x2f80d338}, store_order: 53',
                         'width: 4, source_unchanged_before_load: false', 'extent: 68, needs_bounds: true',
                         'destination_alignment: 4', 'destination_bounds: Some((0, 4294967227))'):
            assert required in row[2], (required, row)
    assert ['word-application', 'Err(DestinationBounds)'] in first_copy_rows
    instance = one(r for r in first_copy_rows if r[0] == 'word-instance')
    assert instance[1:3] == ['0x5006bbca', '0x5006c6ae'] and instance[-1] == 'Some(DestinationBounds)'
    assert f'destination: Base {{ base: Stack {{ function: {0x5006bbca}, offset: -280 }}, offset: 44 }}' in instance[3]
    assert 'source: Base { base: Argument(0), offset: 1408 }' in instance[3]
    event_rows = [r for r in first_copy_rows if r[0] == 'word-event']
    body_events = [r[2] for r in event_rows if r[1] == '0x2f80d260']
    assert body_events == [r[2] for r in event_rows if r[1] == '0x2f8004c4']
    events = []
    for text in body_events:
        m = re.fullmatch(r'WordEvent \{ owner: (\d+), path: (\d+), order: (\d+), site: (\d+), kind: (\w+), width: (\d+), address: (.*), read: (.*) \}', text)
        assert m, text
        owner, path, order, pc, kind, width, address, read = m.groups()
        assert int(owner) == 0x2f80d260
        events.append({'path': int(path), 'order': int(order), 'pc': hex(int(pc)),
                       'kind': kind, 'width': int(width), 'address': address, 'read': read})
    byte_proofs = []
    for path in sorted({event['path'] for event in events}):
        trace = [event for event in events if event['path'] == path]
        assert trace[-1]['kind'] == 'Return'
        assert all(a['order'] < b['order'] for a, b in zip(trace, trace[1:]))
        stores = [event for event in trace if event['kind'] == 'Store']
        for byte in range(44, 48):
            writers = []
            for event in stores:
                address = re.fullmatch(r'Base \{ base: Argument\(0\), offset: (\d+) \}', event['address'])
                assert address, 'a different/unknown base could overlap the selected word'
                offset = int(address[1])
                if byte in {(offset + n) & 0xffffffff for n in range(event['width'])}:
                    writers.append(event)
            assert writers, 'all four bytes need an actual final writer'
            last = writers[-1]
            assert (last['pc'], last['order'], last['width']) == ('0x2f80d338', 53, 4)
            assert last['read'] == f'Some(({0x2f80d332}, 51, 4))'
            load = one(event for event in trace if event['pc'] == '0x2f80d332' and event['order'] == 51)
            assert load['address'] == 'Base { base: Argument(1), offset: 44 }' and load['width'] == 4
            byte_proofs.append({'path': path, 'destination_byte': byte, 'source_byte': byte,
                                'load': load['pc'], 'load_order': 51,
                                'last_writer': last['pc'], 'store_order': 53,
                                'return': trace[-1]['pc']})

    # The real initializer's reloads already have unknown values at their
    # definitions. Noninterference cannot create a captured pointer value.
    slots = []
    for slot, definition, reload in [(-532, 0x5006bd74, 0x5006d1c2),
                                     (-556, 0x5006c368, 0x5006d1c0),
                                     (-524, 0x5006bd98, 0x5006d1aa)]:
        address = f'EntryStack {{ offset: {slot} }}'
        constructor.memory(definition, 'Store', address, 'Some(Unknown)')
        constructor.memory(reload, 'Load', address, 'None')
        slots.append({'slot': slot, 'definition': constructor.snapshot(definition),
                      'reload': constructor.snapshot(reload),
                      'all_same_slot_accesses': [r[1:] for r in constructor.rows if r[0] == 'memory' and r[4] == address],
                      'status': 'unknown: definition contains no captured pointer; same-slot accesses are not an alias or reaching-definition proof'})
    for pc, instruction in [(0x5006bd20, 'lw s9, 0(a0)'),
                            (0x5006bd70, 'lw a0, 260(s9)'),
                            (0x5006bd88, 'lw ra, 276(s9)'),
                            (0x5006bd94, 'lw a0, 292(s9)')]:
        constructor.instruction(pc, instruction)
        constructor.memory(pc, 'Load', 'Unknown', 'None')
    constructor.instruction(0x5006d1c4, 'sw a2, 0(a5)')
    constructor.instruction(0x5006d1f6, 'sw a5, 0(s1)')
    for pc in (0x5006d1c4, 0x5006d1f6):
        write = one(constructor.at('write-event', pc))[0]
        assert 'address: Base { base: Unknown, offset: 0 }' in write
        assert 'value: Base { base: Unknown, offset: 0 }' in write
        assert 'occurrence: May, proof: ClosedGraph' in write
    constructor.reached(0x5006c590, 0x5006c592, 0x5006d1c4, 0x5006d1f6)

    historical_call = 0x5004db66
    run.instruction(int(run.owner[0], 16), 'addi sp, sp, -256')
    assert run.inputs(historical_call)[12] == 'Constant { value: 20 }'
    assert run.at('transfer', historical_call) == [[f'Some(ImageAddress {{ address: {0x2f8004c4} }})']]
    assert not any(r[0] == 'memory' and r[4] == 'EntryStack { offset: -600 }' for r in run.rows)
    assert run.owner != constructor.owner
    effects = capture('short-copy.tsv', 'stack-call-writes', args.elf, args.rom, summaries, f'{historical_call:x}', '-600')
    effect_rows = [line.split('\t') for line in effects.read_text().splitlines()]
    assert not any(r[0] in ('before-spill', 'after-spill', 'before-reload', 'after-reload') for r in effect_rows)
    retained = [r for r in effect_rows if r[0] == 'context-call-write']
    assert len(retained) == 5
    assert {r[1] for r in retained} == {'0x2f80d27a', '0x2f80d2c8', '0x2f80d338'}

    proof = {
        'base': 'main ab2d9f96, including #199 through #205',
        'inputs': {str(p): sha(p) for p in (args.elf, args.rom, report, summaries, chip)},
        'validation_sources': {p.name: sha(p) for p in (Path(__file__), Path(__file__).with_name('explain.py'))},
        'diagnostic_binaries': {name: sha(args.examples / name) for name in ('function-records', 'callback-audit', 'stack-call-writes')},
        'streams': {p.name: sha(p) for p in streams},
        'irq_binding': {'table_entry': entry, 'context': irq_context, 'root': root.snapshot(0x2f004292), 'status': 'proved: immutable IRQ binding and direct tail to doorbell'},
        'handler_load_chain': [handler.snapshot(pc) for pc, *_ in load_specs],
        'indirect_call': handler.snapshot(call),
        'callback_producer': producer, 'callback_field': field,
        'audited_candidate_count': len(candidates),
        'callback_status': 'proved: nested load computations, unique reaching pend load and field type; unknown: concrete mutable objects and exact runtime target',
        'static_initializer': {'word': initializer, 'address_use': address_use, 'typed_storage': static_use,
                               'target_owner': pend.owner, 'status': 'proved ELF initializer; unknown runtime equality at this call'},
        'constructor_stores': [constructor.snapshot(pc) for pc in (0x5006c590, 0x5006c592, 0x5006c5ee, 0x5006c602, 0x5006c606, 0x5006c6f4)],
        'aggregate_copy_calls': copies,
        'first_copy_effect_check': {'call': '0x5006c6ae', 'destination_word_coordinate': -236,
                                    'coordinate_meaning': 'copy destination + 44; not an existing captured spill',
                                    'context': one(r[1:] for r in first_copy_rows if r[0] == 'context'),
                                    'applied_effects': 14, 'preserves_word': False,
                                    'status': 'conditional word contents proved separately; numerical destination bounds and the complete generic footprint remain unknown'},
        'word_copy': {'conditional_postconditions': word_rows, 'events': events,
                      'conditional_caller_instance': instance,
                      'byte_proofs_under_conditions': byte_proofs,
                      'destination_alignment': 4, 'destination_bounds': [0, 0xffffffff - 68],
                      'application': 'Err(DestinationBounds)',
                      'status': 'conditional load-time provenance proved; actual call application remains unknown because numeric entry-sp bounds are absent',
                      'publisher_equality': 'unknown: no separate source-preservation/nonalias proof',
                      'wrapping_counterexample': 'D=0xffffffc0, length=68: after bulk 36, destination pointer 0xffffffe4 is not unsigned-below end 4; the word at D+44 is not written'},
        'conditional_copy_layout': {
            'status': 'unknown: arithmetic under unproved copy-content and object-forwarding premises; never supplied to the analyzer',
            'publisher': 'E+0x580 -> entry-sp-236 -> E+0x13c -> E+0x3bc = (E+0x2b8)+0x104',
            'line_slot': 'E+0x590 -> entry-sp-220 -> E+0x14c -> E+0x3cc = (E+0x2b8)+0x114',
            'cell': 'E+0x15c -> E+0x3dc = (E+0x2b8)+0x124',
            'E': 'rx-poll entry a0, a symbolic object base, not an exact runtime object address'},
        'future_pointer_stores': [constructor.snapshot(pc) for pc in (0x5006bcd0, 0x5006bcd8)],
        'constructor_pointer_loads': [constructor.snapshot(pc) for pc in (0x5006bd20, 0x5006bd70, 0x5006bd88, 0x5006bd94)],
        'actual_slots': slots,
        'line_field_and_publication': [constructor.snapshot(pc) for pc in (0x5006d1c4, 0x5006d1f6)],
        'source_only_connection': 'Arena::bind -> WifiIrqLineParts::install -> RX_LINE -> handler; equality of the published objects is not proved by machine analysis',
        'first_barrier': {'pc': '0x2f00369e', 'missing': 'all-path reaching mutable-memory definition equating *(RX_LINE) with the initialized WifiIrqLine object',
                          'existing_mechanisms': 'common values/FunctionIndex and FunctionWrites/CallWrites; target producer and callback field discovery already work',
                          'constructor_prerequisite': '0x5006d1f6 has neither a proved RX_LINE address nor a concrete line pointer; real spill definitions are already unknown',
                          'first_copy_prerequisite': '0x5006c6ae: conditional four-byte provenance exists, but the numerical destination bound is not proved for entry-sp-280',
                          'next_change': 'derive a numerical stack/address interval from existing checked machine context before applying the conditional word postcondition; source equality, larger copies and publication remain separate obligations'},
        'short_copy_check': {'call': run.snapshot(historical_call), 'frame_bytes': 256, 'historical_slot': -600, 'slot_accesses': [],
                             'contextual_retained_effects': retained,
                             'conclusion': 'the 20-byte call is not the demonstrated pointer-preservation barrier on this chain: it has a different owner and no selected spill/reload; actual constructor definitions are already unknown. Its remaining unknown stores still do not prove general noninterference'},
    }
    (args.output / 'current-chain.json').write_text(json.dumps(proof, indent=2) + '\n')
    print(json.dumps({'irq_call': hex(call), 'loads': len(load_specs), 'candidates': len(candidates),
                      'first_barrier': proof['first_barrier']['pc'], 'historical_slot_accesses': 0}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--examples', type=Path, default=Path('target/tools/examples'))
    validate(parser.parse_args())
