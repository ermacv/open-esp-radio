#!/usr/bin/env python3
"""Check the current-main publisher barrier using only the public diagnostic."""
import argparse
import hashlib
import json
import re
from pathlib import Path
import subprocess


ROM_SHA256 = 'a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--executable', type=Path, default=Path('target/tools/examples/stack-call-writes'))
    args = parser.parse_args()
    assert hashlib.sha256(args.rom.read_bytes()).hexdigest() == ROM_SHA256, 'ROM differs from platform pin'
    command = [str(args.executable), str(args.elf), str(args.rom),
               'chips/esp32s31/platform/rom-functions.toml', '5004dd24', '-600']
    text = subprocess.check_output(command, text=True)
    rows = [line.split('\t') for line in text.splitlines()]

    def one(kind, pc):
        found = [r[2:] for r in rows if r[:2] == [kind, pc]]
        assert len(found) == 1, (kind, pc, found)
        return '\t'.join(found[0])

    # These PCs select this freshly rediscovered fixture, never production policy.
    assert '796918980' in one('target', '0x5004dd24')  # 0x2f8004c4
    inputs = one('inputs', '0x5004dd24')
    assert 'EntryStack { offset: -848 }' in inputs
    assert 'EntryStack { offset: -400 }, Expression' in inputs
    assert 'Constant { value: 128 }' in inputs
    assert '__call_memcpy\tclosed=true' in one('callee', '0x2f8004c4')
    assert 'call=false\tImageAddress { address: 796971616 }' == one('rom-transfer', '0x2f8004c4')
    assert 'memcpy\tclosed=true\tunknown-effects=true' == one('callee', '0x2f80d260')
    # Fingerprint the exact complete instruction inventory emitted by the
    # shared decoder; do not parse ELF sections or decode instructions here.
    body = bytearray()
    for r in rows:
        if r[0] == 'instruction' and 0x2f80d260 <= int(r[1], 16) < 0x2f80d340:
            assert int(r[1], 16) == 0x2f80d260 + len(body)
            body.extend(int(b, 16) for b in re.findall(r'\b[0-9a-f]{2}\b', r[2]))
    assert len(body) == 224
    transfers = [r for r in rows if r[0] == 'rom-transfer']
    assert len(transfers) == 1, transfers  # body is a leaf, trampoline tail only
    assert not [r for r in rows if r[0] == 'sp-write']
    writes = [r for r in rows if r[0] == 'rom-write']
    assert len(writes) == 12
    assert all('address: Base { base: Unknown' in r[2] for r in writes)
    assert {r[1]: int(r[3]) for r in rows if r[0] == 'rom-memory' and r[2] == 'Store'} == {
        '0x2f80d27a': 1, '0x2f80d2c8': 1,
        **{f'{pc:#010x}': 4 for pc in (0x2f80d2f8, 0x2f80d2fc, 0x2f80d304,
                                     0x2f80d308, 0x2f80d30c, 0x2f80d310,
                                     0x2f80d314, 0x2f80d318, 0x2f80d324, 0x2f80d338)},
    }
    assert one('preserves', '0x5004dd24') == 'slot=-600\tfalse'
    spill = one('after-spill', '0x5004dcf2')
    assert 'offset: -600' in spill and f'read: {0x5004d856}' in spill
    assert 'displacement: 260, width: 4' in spill
    assert spill == one('before-spill', '0x5004dcf2')
    reload = one('after-reload', '0x5004eb44')
    assert 'value: Base { base: Unknown' in reload
    assert reload == one('before-reload', '0x5004eb44')
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'facts.tsv').write_text(text)
    result = {
        'baseline': '44324870',
        'elf_sha256': hashlib.sha256(args.elf.read_bytes()).hexdigest(),
        'rom_sha256': ROM_SHA256,
        'rom_body_code_sha256': hashlib.sha256(body).hexdigest(),
        'caller': '0x5004d7c8', 'load': '0x5004d856', 'spill': '0x5004dcf2',
        'call': '0x5004dd24', 'trampoline': '0x2f8004c4', 'body': '0x2f80d260',
        'reload': '0x5004eb44', 'slot': [-600, -596],
        'actual_inputs': inputs,
        'spill_fact': spill, 'reload_fact': reload,
        'preserved': False,
        'rom_store_sites': [r[1] for r in writes],
        'conservative_write_footprint': 'all RV32 addresses; loop-carried store bases remain unknown',
        'first_remaining_barrier': {
            'site': '0x5004dd24',
            'reason': 'trampoline ExternalTransfer/Call; child memcpy has 12 UnknownStore sites',
            'missing_fact': 'a complete bounded footprint for all loop-carried memcpy stores, transported through its tail trampoline',
        },
        'additional_contract_enabled': False,
        'conditional_only': 'If a complete writes-only-[a0,a0+a2) contract were supplied, the substituted destination [-400,-272) would be disjoint from [-600,-596). This is not a derived machine proof.',
    }
    (args.output / 'real-transition.json').write_text(json.dumps(result, indent=2) + '\n')
    print('Pinned target and complete store inventory checked; missing loop footprint retained; publisher reload stays unknown.')


if __name__ == '__main__':
    main()
