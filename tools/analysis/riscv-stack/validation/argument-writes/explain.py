#!/usr/bin/env python3
"""Check the pinned w1 call through the public summary diagnostic, never resolver policy."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def capture(executable, elf, old):
    command = [str(executable), str(elf), '0x5006e6e2']
    if old:
        command.append('--no-symbolic-spills')
    output = subprocess.check_output(command, text=True)
    rows = {}
    for line in output.splitlines():
        parts = line.split('\t', 2)
        if len(parts) == 3:
            rows.setdefault((parts[0], parts[1]), []).append(parts[2])
    return output, rows


def row(rows, kind, pc):
    found = rows[(kind, pc)]
    assert len(found) == 1, (kind, pc, found)
    return found[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--executable', type=Path, default=Path('target/tools/examples/call-writes'))
    args = parser.parse_args()
    before_text, before = capture(args.executable, args.elf, True)
    after_text, after = capture(args.executable, args.elf, False)
    args.output.mkdir(parents=True, exist_ok=True)
    for name, text in [('before.tsv', before_text), ('after.tsv', after_text)]:
        (args.output / name).write_text(text)
    # Names and PCs select this diagnostic fixture only. Production summary
    # semantics use canonical machine records and explicit integer ABI.
    assert 'closed=true' in row(after, 'callee', '0x5004d7c8')
    irq = row(after, 'write', '0x5004d8aa')
    assert 'address: Base { base: Argument(0), offset: 208 }' in irq
    assert f'read: {0x5004d846}' in irq and 'displacement: 228, width: 4' in irq
    assert 'occurrence: May, proof: ClosedGraph' in irq
    publisher = row(after, 'write', '0x5004dcf2')
    assert 'offset: -600' in publisher and f'read: {0x5004d856}' in publisher
    assert 'displacement: 260, width: 4' in publisher
    # The real successful spill/reload example is an exit-report word. Do not
    # relabel it as a pointer or substitute it for the IRQ publisher witness.
    reload_before = row(before, 'reload', '0x5004da5a')
    reload_after = row(after, 'reload', '0x5004da5a')
    assert 'value: Base { base: Unknown' in reload_before
    assert 'slot: -384, width: 4, value: Field' in reload_after
    assert f'read: {0x5004da18}' in reload_after
    spill = row(after, 'write', '0x5004da2e')
    assert 'offset: -384' in spill and f'read: {0x5004da18}' in spill
    final_before = row(before, 'write', '0x5004da5e')
    final_after = row(after, 'write', '0x5004da5e')
    assert 'value: Base { base: Unknown' in final_before
    assert 'value: Field' in final_after and f'read: {0x5004da18}' in final_after
    assert 'occurrence: May' in final_after
    install = row(after, 'write', '0x5004eb7a')
    assert 'value: Base { base: Unknown' in install and 'occurrence: May' in install
    assert 'Call' in after[('barrier', '0x5004dd24')]
    assert 'value: Base { base: Unknown' in row(after, 'reload', '0x5004eb44')
    applied_irq = row(after, 'applied', '0x5004d8aa')
    assert 'base: Unknown, displacement: 228' in applied_irq
    assert f'Address({0x2f007004})' not in applied_irq
    evidence = {
        'baseline': '4072d86a652d31bab14fc5fc0b27112ea198051f',
        'elf_sha256': hashlib.sha256(args.elf.read_bytes()).hexdigest(),
        'caller': '0x5006e58e', 'call': '0x5006e6e2', 'callee': '0x5004d7c8',
        'abi': 'RV32 integer register words; a0 is a future pointer, a1 a Context pointer',
        'call_inputs': row(after, 'inputs', '0x5006e6e2'),
        'irq_storage_pointer': irq,
        'irq_storage_call_effect': applied_irq,
        'line_mac_pointer_to_spill': publisher,
        'spill_example_kind': 'WifiRxExit payload word; not a proved pointer',
        'spill_example': {'spill': spill, 'reload_before': reload_before,
                         'reload_after': reload_after, 'store_before': final_before,
                         'store_after': final_after},
        'install': install,
        'remaining_barriers': [
            'The actual caller a0 value is unknown; no exact future/handler object is substituted.',
            'The line.mac spill meets unknown call effects at 0x5004dd24 before reload.',
            'The conditional HandlerCell pointer store remains unknown.',
            'No complete writes/initial-state/alias/publication-order proof exists for the later handler read.',
        ],
    }
    (args.output / 'real-transition.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print('Real IRQ pointer field/store fact and separate spill-word witness checked; RX_LINE barrier retained.')


if __name__ == '__main__':
    main()
