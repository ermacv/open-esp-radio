#!/usr/bin/env python3
"""Replay saved diagnostic streams through the current validator with bad facts.

This tests the evidence validator only; it never provides production premises.
The Rust regressions separately exercise the checked application factories.
"""
import argparse
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import current_chain


def replace(text, before, after):
    assert before in text, before
    return text.replace(before, after)


def later_store(text):
    lines = text.splitlines()
    for target in ('0x2f80d260', '0x2f8004c4'):
        prefix = f'word-event\t{target}\t'
        index = next(i for i, line in enumerate(lines) if line.startswith(prefix) and 'kind: Return' in line)
        event = lines[index].split('\t', 2)[2]
        order = int(event.split('order: ')[1].split(',')[0])
        lines[index] = lines[index].replace(f'order: {order},', f'order: {order + 1},')
        inserted = (prefix + 'WordEvent { owner: 796971616, path: 0, order: ' + str(order)
                    + ', site: 796971834, kind: Store, width: 4, address: Base { base: Argument(0), offset: 44 }, read: None }')
        lines.insert(index, inserted)
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('reference', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--examples', type=Path, default=Path('target/tools/examples'))
    args = parser.parse_args()
    cases = {
        'target_load_width': ('doorbell.tsv', lambda s: replace(s, 'memory\t0x2f00371c\tLoad\t4\t', 'memory\t0x2f00371c\tLoad\t2\t')),
        'captured_slot': ('rx-poll.tsv', lambda s: replace(s, 'memory\t0x5006bcf0\tStore\t4\tEntryStack { offset: -532 }\tSome(Unknown)', 'memory\t0x5006bcf0\tStore\t4\tEntryStack { offset: -532 }\tSome(Constant { value: 7 })')),
        'source_load_width': ('first-constructor-copy.tsv', lambda s: replace(s, 'kind: Load, width: 4, address: Base { base: Argument(1), offset: 44 }', 'kind: Load, width: 2, address: Base { base: Argument(1), offset: 44 }')),
        'later_overlapping_store': ('first-constructor-copy.tsv', later_store),
        'forged_application': ('first-constructor-copy.tsv', lambda s: replace(s, 'word-application\tErr(DestinationBounds)', 'word-application\tOk(forged)')),
        'changed_setup_pointer': ('first-constructor-copy.tsv', lambda s: replace(s, 'pointer: 1343076800, scratch:', 'pointer: 1343076816, scratch:')),
        'rebased_entry_pointers': ('first-constructor-copy.tsv', lambda s: replace(s, 'basis: Installed { previous_coordinate: 0 }', 'basis: Entry')),
        'omitted_cpu0_context': ('first-constructor-copy.tsv', lambda s: replace(s, 'stack-incoming\t0x50019a18\t0x50019c1c\t0x5006bb46\tSome({TaskPolls})\n', '')),
        'omitted_open_cfg_edge': ('first-constructor-copy.tsv', lambda s: '\n'.join(l for l in s.splitlines() if not l.startswith('stack-context-open\t0x500654f6\t')) + '\n'),
        'forged_interval': ('first-constructor-copy.tsv', lambda s: replace(s, 'stack-context-result\tNone', 'stack-context-result\tSome((0, 1343076800))')),
    }
    results = {}
    for name, mutation in [('positive_replay', None), *cases.items()]:
        output = args.output / name

        def replay(_command, *, stdout, check):
            assert check
            stream = Path(stdout.name).name
            data = (args.reference / stream).read_text()
            if mutation and stream == mutation[0]:
                data = mutation[1](data)
            stdout.write(data)
            return SimpleNamespace(returncode=0)

        try:
            with patch.object(current_chain.subprocess, 'run', replay), redirect_stdout(io.StringIO()):
                current_chain.validate(SimpleNamespace(
                    elf=args.elf, rom=args.rom, output=output, examples=args.examples))
        except AssertionError:
            assert mutation is not None, 'positive replay failed'
            results[name] = 'rejected'
        else:
            assert mutation is None, f'bad fact accepted: {name}'
            results[name] = 'passed'
    results['actual_call_application'] = 'rejected: DestinationBounds; numerical context remains incomplete'
    (args.output / 'current-chain-checks.json').write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
