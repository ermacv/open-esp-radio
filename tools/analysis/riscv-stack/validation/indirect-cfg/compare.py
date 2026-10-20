#!/usr/bin/env python3
"""Compare all machine bytes, reports and full callback audits without allowlists."""
import argparse
import difflib
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    'bundle_comparison', Path(__file__).resolve().parents[1] / 'compare-callback-audit.py')
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    result = shared.compare(args.before, args.after)
    for app in ('w0', 'w1'):
        for stage in ('runtime', 'bootstrap'):
            name = f'{app}-{stage}-callbacks.tsv'
            before = (args.before / name).read_text().splitlines()
            after = (args.after / name).read_text().splitlines()
            result[app]['diagnostics'][stage]['full_audit_diff'] = list(difflib.unified_diff(
                before, after, fromfile='before/' + name, tofile='after/' + name, lineterm=''))
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
