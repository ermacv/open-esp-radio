#!/usr/bin/env python3
"""Validate the fresh main dispatch and subsequent CPU0 stack-context capture.

Fixture addresses select diagnostic input only. Production proofs come from
the common evaluator, table source, CFG and checked stack-context factories.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import subprocess

SITE = '0x50065a40'
OWNER = '0x5006553a'
ELF_SHA = 'dc1ff3ae9e5e007439d5b15310d7b6243276efae1b7dad90873f2e755f357571'
ROM_SHA = 'a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def one(rows):
    rows = list(rows)
    assert len(rows) == 1, rows
    return rows[0]


def check(streams):
    main = streams['main.tsv']
    stack = streams['stack.tsv']
    assert one(r for r in main if r[0] == 'function')[1] == OWNER
    assert ['cfg-closed', 'true'] in main
    assert one(r for r in main if r[0] == 'write-summary')[1] == 'closed=true'
    assert one(r for r in main if r[0] == 'dispatch' and r[1] == SITE)[2:] == [
        f'Constant {{ value: {0x5009f3a0} }}', 'Unknown', '2']
    assert f'Constant {{ value: {0x5009f3a0} }}' in one(
        r for r in main if r[:2] == ['dispatch-inputs', SITE])[2]
    # Every followed target is exactly a word of the existing relocated,
    # immutable compiler table. No edge is supplied by this validator.
    words = [r for r in main if r[:2] == ['dispatch-word', SITE]]
    assert len(words) == 30
    entries = []
    for i, row in enumerate(words):
        assert int(row[2], 16) == 0x5009f3a0 + 4 * i
        value = int(row[3], 16)
        assert row[4:] == [f'Some({value})', f'Some({value})']
        entries.append(value & 0xfffffffe)
    edges = []
    for row in main:
        if row[0] == 'graph' and row[1].startswith(f'Edge {{ from: {int(SITE, 16)},'):
            match = re.fullmatch(r'Edge \{ from: \d+, target: Some\((\d+)\), relation: Jump, external: false \}', row[1])
            assert match, row
            edges.append(int(match[1]))
    assert edges == sorted(set(entries)) and len(edges) == 30
    instructions = {r[1]: r[2] for r in main if r[0] == 'instruction'}
    for pc, text in [
        ('0x50065a2e', 'lbu a5, 0(a3)'), ('0x50065a32', 'slli a5, a5, 2'),
        ('0x50065a34', 'add a0, s9, a5'), ('0x50065a38', 'lw a0, 96(a0)'),
        ('0x50065a3a', 'bne a0, zero, -20'), ('0x50065a3c', 'add a5, a5, a4'),
        ('0x50065a3e', 'lw a0, 0(a5)'), (SITE, 'jalr zero, 0(a0)'),
    ]:
        assert instructions[pc] == text
    assert not any(r[0] == 'stack-context-open' and r[1] == OWNER for r in stack)
    setup = one(r[1] for r in stack if r[0] == 'stack-setup')
    assert 'pointer: 1343076928, scratch: 788591808' in setup
    resumed = one(r[1] for r in stack if r[0] == 'stack-resumed')
    assert 'basis: Installed { previous_coordinate: 0 }' in resumed
    root = one(r for r in stack if r[0] == 'stack-root')
    assert root[1] == 'runtime_main' and 'basis: Entry' in root[2]
    assert root[2].startswith('Some(StackContext { owner: 1342656046, start: 1342656046, basis: Entry, interval: (1343076928, 1343076928),')
    incoming = [r for r in stack if r[0] == 'stack-incoming' and r[3] == '0x5006bb8a']
    assert {(r[1], r[2], r[4]) for r in incoming} == {
        ('0x2f00337e', '0x2f003490', 'Some({TaskPolls})'),
        ('0x50019a18', '0x50019c1c', 'Some({TaskPolls})'),
    }
    expected = [
        ('0x50074e2e', '0x50074e32', 0x5006553a, 0, 0x500dba40),
        (OWNER, '0x50069490', 0x50019a18, -2384, 0x500db0f0),
        ('0x50019a18', '0x50019c1c', 0x5006bb8a, -80, 0x500db0a0),
    ]
    steps = []
    for caller, site, callee, delta, interval in expected:
        row = one(r for r in stack if r[:3] == ['stack-context-step', caller, site])
        assert row[3:5] == ['closed=true', f'sp=Some(EntryStack {{ offset: {delta} }})']
        assert row[5].startswith(f'Some(StackContext {{ owner: {callee}, start: {callee}, basis: Entry, interval: ({interval}, {interval}),')
        steps.append({'caller': caller, 'site': site, 'relative_sp': delta,
                      'callee_entry_sp': f'{interval:#010x}', 'proof': row[5]})
    assert 'assumptions: [ExecutorInvariant]' in steps[-1]['proof']
    irq = one(r for r in stack if r[:3] == ['stack-context-step', '0x2f00337e', '0x2f003490'])
    assert irq[5] == 'None'
    assert ['stack-context-result', 'None'] in stack
    assert ['word-application', 'Err(DestinationBounds)'] in stack
    assert any(r[0] == 'stack-irq-gap' for r in stack)
    return {'owner': OWNER, 'site': SITE, 'closed_by_common_check': True,
            'table_base': '0x5009f3a0', 'relocated_read_only_words': words,
            'targets': [f'{t:#010x}' for t in edges], 'cpu0_steps': steps,
            'remaining': {'irq_context': 'unknown', 'merged_rx_poll_context': None,
                          'application': 'Err(DestinationBounds)'}}


def replay_checks(streams):
    cases = {
        'missing_target_edge': ('main.tsv', lambda rows: rows.remove(one(r for r in rows if r[0] == 'graph' and r[1].startswith(f'Edge {{ from: {int(SITE, 16)},') and f'target: Some({0x50065a42})' in r[1]))),
        'changed_table_word': ('main.tsv', lambda rows: one(r for r in rows if r[:3] == ['dispatch-word', SITE, '0x5009f3a0']).__setitem__(3, '0x50065a43')),
        'omitted_irq_path': ('stack.tsv', lambda rows: rows.remove(one(r for r in rows if r[:3] == ['stack-incoming', '0x2f00337e', '0x2f003490']))),
        'forged_cpu0_context': ('stack.tsv', lambda rows: one(r for r in rows if r[:3] == ['stack-context-step', OWNER, '0x50069490']).__setitem__(5, 'None')),
        'forged_cpu0_interval': ('stack.tsv', lambda rows: one(r for r in rows if r[:3] == ['stack-context-step', OWNER, '0x50069490']).__setitem__(5, re.sub(r'interval: \(\d+, \d+\)', 'interval: (0, 0)', one(r for r in rows if r[:3] == ['stack-context-step', OWNER, '0x50069490'])[5], count=1))),
        'rebased_installer_context': ('stack.tsv', lambda rows: one(r for r in rows if r[0] == 'stack-resumed').__setitem__(1, one(r for r in rows if r[0] == 'stack-resumed')[1].replace('basis: Installed { previous_coordinate: 0 }', 'basis: Entry'))),
        'forged_merged_context': ('stack.tsv', lambda rows: one(r for r in rows if r[0] == 'stack-context-result').__setitem__(1, 'Some(forged)')),
        'forged_application': ('stack.tsv', lambda rows: one(r for r in rows if r[0] == 'word-application').__setitem__(1, 'Ok(forged)')),
    }
    results = {'positive_replay': 'passed'}
    for name, (stream, mutate) in cases.items():
        altered = copy.deepcopy(streams)
        mutate(altered[stream])
        try:
            check(altered)
        except AssertionError:
            results[name] = 'rejected'
        else:
            raise AssertionError(f'bad fact accepted: {name}')
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--examples', type=Path, default=Path('target/tools/examples'))
    parser.add_argument('--replay', type=Path)
    args = parser.parse_args()
    assert sha(args.elf) == ELF_SHA
    assert sha(args.rom) == ROM_SHA
    args.output.mkdir(parents=True, exist_ok=True)
    commands = {
        'main.tsv': ['function-records', args.elf, SITE, '--writes', '--image-facts'],
        'stack.tsv': ['stack-call-writes', args.elf, args.rom,
                      'chips/esp32s31/platform/rom-functions.toml', '5006c66e', '-236',
                      '--word-offset', '44', '--checked-stack-context'],
    }
    streams, hashes = {}, {}
    for name, command in commands.items():
        path = (args.replay or args.output) / name
        if not args.replay:
            with path.open('w') as out:
                subprocess.run([str(args.examples / command[0]), *map(str, command[1:])], stdout=out, check=True)
        streams[name] = [line.split('\t') for line in path.read_text().splitlines()]
        hashes[name] = sha(path)
    proof = {'base': 'main 4901ee7d (#222, #223, #216, #221; including 6ed51a2b)',
             'elf_sha256': ELF_SHA, 'rom_sha256': ROM_SHA,
             'streams_sha256': hashes, **check(streams), 'replay_checks': replay_checks(streams)}
    (args.output / 'proof.json').write_text(json.dumps(proof, indent=2) + '\n')
    print(json.dumps({'targets': len(proof['targets']), 'cpu0_rx_poll_entry_sp': '0x500db0a0',
                      'remaining': proof['remaining'], 'replay_checks': proof['replay_checks']}, indent=2))


if __name__ == '__main__':
    main()
