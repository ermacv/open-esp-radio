#!/usr/bin/env python3
"""Compare saved fw bundles and callback-audit TSVs, without allowlists."""
import argparse
from collections import Counter, defaultdict
import difflib
import hashlib
import json
from pathlib import Path


def diagnostic(path):
    sites, reasons, counts = {}, defaultdict(set), {}
    for line in path.read_text().splitlines():
        row = line.split('\t')
        if row[0] == 'site':
            sites[(row[1], row[2])] = set()
        elif row[0] == 'candidate':
            sites.setdefault((row[1], row[2]), set()).add(row[3])
        elif row[0] == 'use' and not row[-1].startswith('typed:'):
            reasons[row[1]].add(row[-1])
        elif row[0] == 'unknown-class':
            counts[row[1]] = {'uses': int(row[2]), 'missing_fact': row[3]}
        elif row[0] in ('typed-uses', 'no-uses'):
            counts[row[0]] = int(row[1])
    return sites, reasons, counts


def compare(before, after):
    result = {}
    for app in ('w0', 'w1'):
        entry = result[app] = {'machines': {}, 'reports': {}, 'diagnostics': {}}
        old, new = before / app, after / app
        machines = sorted(p.name for p in old.iterdir() if p.suffix in ('.elf', '.bin'))
        assert len(machines) == 7 and machines == sorted(p.name for p in new.iterdir() if p.suffix in ('.elf', '.bin'))
        for name in machines:
            a, b = (old / name).read_bytes(), (new / name).read_bytes()
            entry['machines'][name] = {'identical': a == b, 'sha256': hashlib.sha256(b).hexdigest()}
            assert a == b, f'{app}/{name} changed machine bytes'
        for name in ('placement.txt', 'runtime-stack.txt', 'interrupt-stack.txt', 'bootstrap-stack.txt'):
            a, b = (old / name).read_text().splitlines(), (new / name).read_text().splitlines()
            entry['reports'][name] = {
                'identical': a == b,
                'semantic_identical': [l for l in a if not l.startswith('analysis: ')] == [l for l in b if not l.startswith('analysis: ')],
                'diff': list(difflib.unified_diff(a, b, fromfile='before/' + name, tofile='after/' + name, lineterm='')),
            }
        for stage in ('runtime', 'bootstrap'):
            filename = f'{app}-{stage}-callbacks.tsv'
            a, _, _ = diagnostic(before / filename)
            b, reasons, counts = diagnostic(after / filename)
            out = entry['diagnostics'][stage] = {'unknown_classes': counts, 'resolvers': {}}
            out['full_tsv_identical'] = (before / filename).read_bytes() == (after / filename).read_bytes()
            for resolver in ('global', 'typed'):
                keys = sorted(k for k in a.keys() | b.keys() if k[0] == resolver)
                changed = []
                for key in keys:
                    added, removed = b.get(key, set()) - a.get(key, set()), a.get(key, set()) - b.get(key, set())
                    if added or removed or (key in a) != (key in b):
                        changed.append({'site': key[1], 'added': len(added), 'removed': len(removed),
                                        'added_unknown_reasons': dict(Counter(r for target in added for r in reasons[target]))})
                out['resolvers'][resolver] = {
                    'sites_before': sum(k[0] == resolver for k in a),
                    'sites_after': sum(k[0] == resolver for k in b),
                    'candidate_edges_before': sum(len(v) for k, v in a.items() if k[0] == resolver),
                    'candidate_edges_after': sum(len(v) for k, v in b.items() if k[0] == resolver),
                    'changed_sites': changed,
                }
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    output = json.dumps(compare(args.before, args.after), indent=2) + '\n'
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end='')
