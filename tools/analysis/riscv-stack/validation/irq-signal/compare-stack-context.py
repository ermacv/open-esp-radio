#!/usr/bin/env python3
"""Compare full stack-context captures using the existing bundle comparator."""
import argparse
import difflib
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    'callback_comparison', Path(__file__).resolve().parents[1] / 'compare-callback-audit.py')
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def compare(before, after):
    result = shared.compare(before, after)
    for app in ('w0', 'w1'):
        for report in result[app]['reports'].values():
            assert report['semantic_identical'], 'stack/placement report semantics changed'
        for stage in ('runtime', 'bootstrap'):
            name = f'{app}-{stage}-callbacks.tsv'
            a = (before / name).read_text().splitlines()
            b = (after / name).read_text().splitlines()
            entry = result[app]['diagnostics'][stage]
            assert all(not r['changed_sites'] for r in entry['resolvers'].values())
            classes = ('use', 'proof', 'unknown-class')
            assert [l for l in a if l.split('\t')[0] not in classes] == [l for l in b if l.split('\t')[0] not in classes]

            def keyed(lines):
                rows = [l.split('\t') for l in lines if l.split('\t')[0] in classes]
                return {tuple(r[:3] if r[0] in ('use', 'proof') else r[:2]): r for r in rows}

            old, new = keyed(a), keyed(b)
            changes = [{'before': old.get(k), 'after': new.get(k)}
                       for k in sorted(old.keys() | new.keys()) if old.get(k) != new.get(k)]
            for change in changes:
                x, y = change['before'], change['after']
                if (x or y)[0] == 'use':
                    assert x[:-1] == y[:-1] and (x[-1], y[-1]) == ('ControlFlow', 'UnknownWrite')
                if (x or y)[0] == 'proof':
                    assert x[:3] == y[:3] and x[4:] == y[4:], 'proof evidence changed beyond its stopping PC'
            entry['full_audit_changes'] = changes
            entry['full_audit_diff'] = list(difflib.unified_diff(
                a, b, fromfile='before/' + name, tofile='after/' + name, lineterm=''))
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(json.dumps(compare(args.before, args.after), indent=2) + '\n')
