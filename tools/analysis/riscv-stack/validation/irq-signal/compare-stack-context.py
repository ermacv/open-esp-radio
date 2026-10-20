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
            assert a == b, 'full callback audit changed outside the opt-in stack profile'
            entry['full_audit_changes'] = []
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
