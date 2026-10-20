#!/usr/bin/env python3
"""Explain PC-relative reclassification and every changed candidate from TSV evidence."""
import argparse
from collections import Counter, defaultdict
import csv
import json
from pathlib import Path
import re


def load(path):
    uses = defaultdict(dict)
    proofs, pairs, sites = {}, {}, {}
    for line in path.read_text().splitlines():
        row = line.split('\t')
        if row[0] == 'use':
            uses[row[1]][int(row[2])] = {'site': row[3], 'role': row[4], 'addend': int(row[5]), 'evidence': row[6]}
        elif row[0] == 'proof':
            barrier = re.fullmatch(r'Some\((\d+)\)', row[3])
            proofs[(row[1], int(row[2]))] = {
                'barrier': f'{int(barrier[1]):#010x}' if barrier else None,
                'stores': re.sub(r'site: (\d+)', lambda m: f'site: {int(m[1]):#010x}', row[4]),
            }
        elif row[0] == 'pcrel':
            pairs[int(row[1])] = dict(zip([
                'low', 'high', 'section', 'low_site', 'low_symbol', 'label_section', 'label_address', 'low_addend',
                'high_site', 'target_symbol', 'target_section', 'target_symbol_address', 'high_addend',
                'normalized_target', 'functions', 'classification'], row[1:]))
        elif row[0] == 'site':
            sites[(row[1], row[2])] = set()
        elif row[0] == 'candidate':
            sites.setdefault((row[1], row[2]), set()).add(row[3])
    for pair in pairs.values():
        pair['functions'] = [f'{int(n):#010x}' for n in re.findall(r'\d+', pair['functions'])]
    return uses, proofs, pairs, sites


def explain(before, after, output):
    output.mkdir(parents=True, exist_ok=True)
    summary = {}
    all_pairs, changed_uses, candidate_changes = [], [], []
    for app in ('w0', 'w1'):
        for stage in ('runtime', 'bootstrap'):
            name = f'{app}-{stage}'
            old, _, _, old_sites = load(before / f'{name}-callbacks.tsv')
            new, proofs, pairs, new_sites = load(after / f'{name}-callbacks.tsv')
            by_high = defaultdict(list)
            counts = Counter()
            for index, pair in pairs.items():
                if pair['high'] != '-':
                    by_high[int(pair['high'])].append(pair)
                previous = [{'function': f, **uses[index]} for f, uses in old.items() if index in uses]
                current = [{'function': f, **uses[index], **proofs.get((f, index), {})} for f, uses in new.items() if index in uses]
                hi = int(pair['high']) if pair['high'] != '-' else -1
                previous_high = [{'function': f, **uses[hi]} for f, uses in old.items() if hi in uses]
                current_high = [{'function': f, **uses[hi], **proofs.get((f, hi), {})} for f, uses in new.items() if hi in uses]
                outcomes = sorted({u['evidence'].split(':', 1)[0] for u in current})
                classification = '/'.join(outcomes) if outcomes else pair['classification']
                counts[classification] += 1
                all_pairs.append({'image': name, **pair, 'before': previous, 'after': current, 'before_high': previous_high, 'after_high': current_high, 'result': classification})
            changes = []
            for function in sorted(old.keys() | new.keys()):
                for index in sorted(old[function].keys() | new[function].keys()):
                    a, b = old[function].get(index), new[function].get(index)
                    if a == b:
                        continue
                    related = [pairs[index]] if index in pairs else by_high[index]
                    changes.append({'image': name, 'function': function, 'index': index, 'before': a, 'after': b,
                                    'proof': proofs.get((function, index)), 'normalized_pairs': related})
            changed_uses.extend(changes)
            added_edges, removed_edges = 0, 0
            removed_targets, added_targets = set(), set()
            for key in sorted(old_sites.keys() | new_sites.keys()):
                added = new_sites.get(key, set()) - old_sites.get(key, set())
                removed = old_sites.get(key, set()) - new_sites.get(key, set())
                added_edges += len(added)
                removed_edges += len(removed)
                removed_targets |= removed
                added_targets |= added
                for function in sorted(removed):
                    # A disappearance requires proof: all former observations
                    # are label operands whose normalized targets are elsewhere.
                    # No function/address list determines this conclusion.
                    assert old[function], (name, function, 'no prior evidence')
                    assert not new[function], (name, function, 'new typed exclusion requires further explanation')
                    lineage = []
                    for index, use in old[function].items():
                        assert use['role'] == 'PcRelativeLow', (name, function, index, use)
                        pair = pairs[index]
                        assert pair['high'] != '-' and pair['normalized_target'] != '-'
                        assert function not in pair['functions'], (name, function, pair)
                        lineage.append(pair)
                    candidate_changes.append({'image': name, 'resolver': key[0], 'site': key[1], 'target': function,
                                              'change': 'removed', 'reason': 'OnlyAuipcLabelOperands', 'pairs': lineage})
                for function in sorted(added):
                    blockers = [{'index': index, **use, **proofs.get((function, index), {})}
                                for index, use in new[function].items() if not use['evidence'].startswith('typed:')]
                    assert blockers, (name, function, 'new candidate has no unknown-use explanation')
                    candidate_changes.append({'image': name, 'resolver': key[0], 'site': key[1], 'target': function,
                                              'change': 'added', 'reason': 'UnknownValuePath', 'uses': blockers})
            summary[name] = {'low_uses': len(pairs),
                             'previous_non_exact_lows': sum(1 for row in all_pairs if row['image'] == name and any(u['evidence'] == 'NonExactAddress' for u in row['before'])),
                             'outcomes': dict(counts), 'changed_function_use_relationships': len(changes),
                             'added_candidate_edges': added_edges, 'removed_candidate_edges': removed_edges,
                             'added_unique_targets': len(added_targets), 'removed_unique_targets': len(removed_targets)}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    for name, rows in [('pairs', all_pairs), ('changed-uses', changed_uses), ('candidate-changes', candidate_changes)]:
        (output / f'{name}.json').write_text(json.dumps(rows, indent=2) + '\n')
    def bindings(rows):
        return '; '.join(f"{r['function']}={r['evidence']}" +
                         (f" barrier={r['barrier']} stores={r['stores']}" if 'barrier' in r else '') for r in rows) or '-'
    # Complete source identities and low/high outcomes, without copying the
    # whole raw observation into each before/after binding.
    with (output / 'pairs.tsv').open('w') as out:
        writer = csv.writer(out, delimiter='\t', lineterminator='\n')
        fields = ['image', 'low', 'high', 'section', 'low_site', 'low_symbol', 'label_section', 'label_address', 'low_addend',
                  'high_site', 'target_symbol', 'target_section', 'target_symbol_address', 'high_addend', 'normalized_target', 'result', 'before', 'after', 'before_high', 'after_high']
        writer.writerow(fields)
        for row in all_pairs:
            writer.writerow([bindings(row[key]) if key in ('before', 'after', 'before_high', 'after_high') else row[key] for key in fields])
    with (output / 'changed-uses.tsv').open('w') as out:
        writer = csv.writer(out, delimiter='\t', lineterminator='\n')
        writer.writerow(['image', 'function', 'use_index', 'site', 'role', 'before', 'after', 'barrier', 'stores', 'normalized_low_indices'])
        for row in changed_uses:
            use = row['after'] or row['before']
            proof = row['proof'] or {}
            writer.writerow([row['image'], row['function'], row['index'], use['site'], use['role'],
                             row['before']['evidence'] if row['before'] else '-', row['after']['evidence'] if row['after'] else '-',
                             proof.get('barrier') or '-', proof.get('stores', '-'), ','.join(p['low'] for p in row['normalized_pairs']) or '-'])
    grouped = {}
    for row in candidate_changes:
        key = (row['image'], row['resolver'], row['change'], row['target'])
        if key not in grouped:
            grouped[key] = {k: v for k, v in row.items() if k not in ('site', 'pairs')}
            grouped[key]['sites'] = []
            if 'pairs' in row:
                grouped[key]['normalized_low_indices'] = [p['low'] for p in row['pairs']]
        grouped[key]['sites'].append(row['site'])
    (output / 'candidate-proofs.json').write_text(json.dumps(list(grouped.values()), indent=2) + '\n')
    return summary


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    print(json.dumps(explain(args.before, args.after, args.output), indent=2))
