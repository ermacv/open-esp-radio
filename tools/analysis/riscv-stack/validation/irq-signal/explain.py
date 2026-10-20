#!/usr/bin/env python3
"""Build the IRQ registry and check every new field's candidate exclusions.

Names classify diagnostic rows only. Candidate proofs use original audit uses,
field identities, and the common audit's own compatibility results.
"""
import argparse
import ast
from collections import defaultdict
import csv
import json
from pathlib import Path
import re
import subprocess

FIELD = re.compile(r'FunctionPointerType \{ subroutine: (\d+), foreign: (true|false) \}')


def fields(text):
    return [(int(die), foreign == 'true') for die, foreign in FIELD.findall(text)]


def diagnostic(path):
    out = {'uses': defaultdict(list), 'sites': {}, 'unresolved': {}, 'loads': defaultdict(list), 'matches': {}}
    for line in path.read_text().splitlines():
        r = line.split('\t')
        if r[0] == 'use':
            out['uses'][r[1]].append({'index': int(r[2]), 'pc': r[3], 'evidence': r[6], 'fields': fields(r[6])})
        elif r[0] == 'site':
            out['sites'][(r[1], r[2])] = set()
        elif r[0] == 'candidate':
            out['sites'][(r[1], r[2])].add(r[3])
        elif r[0] == 'unresolved':
            out['unresolved'][r[1]] = r
        elif r[0] == 'load-field':
            out['loads'][r[1]].append(r)
        elif r[0] == 'field-match':
            out['matches'][(fields(r[1])[0], fields(r[2])[0])] = r[3] == 'true'
    return out


def contexts(report):
    result = defaultdict(list)
    hart, level = None, None
    for line in report.read_text().splitlines():
        if m := re.match(r'hart (\d+):', line):
            hart = int(m[1])
        if m := re.match(r'  level (\d+):', line):
            level = int(m[1])
        if m := re.match(r'    unresolved (.+) at (0x[\da-f]+)', line):
            result[m[2]].append({'hart': hart, 'level': level, 'source': m[1],
                                 'executor': 'P2 interrupt executor' if level == 2 else 'hardware/doorbell handler'})
    return result


def callback_class(chain):
    # Presentation labels never feed an analyzer or a candidate decision.
    joined = ' '.join(chain)
    if 'SignalPend>::pend' in joined:
        return 'IrqSignal.pend'
    if 'IrqSignal>::owned' in joined:
        return 'IrqSignal.running'
    if 'PlatformClockGuard' in joined:
        return 'platform clock release'
    if 'ExternalRxBuffer' in joined:
        return 'external RX release'
    if 'Waker' in joined:
        return 'waker vtable method'
    if 'target_executor' in joined or 'settle::settle' in joined:
        return 'PHY dyn Clock callback'
    return 'unclassified (retain unknown)'


def saved_records(before, app, rows, directory, executable):
    by_function = {r[2]: pc for pc, r in sorted(rows.items(), reverse=True)}
    directory.mkdir(parents=True, exist_ok=True)
    values, names, graphs = {}, {}, {}
    for pc in by_function.values():
        path = directory / f'{pc}.tsv'
        with path.open('w') as out:
            subprocess.run([str(executable), str(before / app / 'runtime.elf'), pc], stdout=out, check=True)
        expressions, targets, graph = {}, {}, []
        function = None
        for line in path.read_text().splitlines():
            r = line.split('\t')
            if r[0] == 'function':
                function = r[2]
            elif r[0] == 'expression':
                expressions[int(r[1])] = {'pc': r[2], 'expression': r[3]}
            elif r[0] == 'target-value':
                targets[r[1]] = (r[2], r[3])
            elif r[0] == 'graph' and ((r[1].startswith('Gap {') and 'Unvisited' not in r[1]) or any(s in r[1] for s in ('relation: Indirect', 'relation: Conflict', 'relation: Stop', 'UnexpandedControlFlow'))):
                graph.append(r[1])
        for site, (register, value) in targets.items():
            # Follow saved expression identities, not a second expression index
            # in an analyzer. This script only formats the FunctionIndex output.
            pending = [int(n) for n in re.findall(r'Expression \{ id: (\d+) \}', value)]
            selected = {}
            while pending:
                ident = pending.pop()
                if ident in selected or ident not in expressions:
                    continue
                expression = expressions[ident]
                selected[ident] = expression
                pending.extend(int(n) for n in re.findall(r'Expression \{ id: (\d+) \}', expression['expression']))
            values[site] = {'register': register, 'value': value, 'expressions': selected}
        for site in rows:
            # Physical function ownership is selected by the engine's containing
            # function; records from another function never stand in for a site.
            if site in targets:
                names[site], graphs[site] = function, graph
    return values, names, graphs


def main(before, after, output, executable):
    output.mkdir(parents=True, exist_ok=True)
    registry, comparisons = [], {}
    for app in ('w0', 'w1'):
        old = diagnostic(before / f'{app}-runtime-callbacks.tsv')
        new = diagnostic(after / f'{app}-runtime-callbacks.tsv')
        assert old['uses'] == new['uses'], 'address-use audit changed outside transport'
        for key, targets in old['sites'].items():
            assert new['sites'][key] == targets, f'existing candidates changed: {key}'
        ctx = contexts(before / app / 'interrupt-stack.txt')
        assert ctx == contexts(after / app / 'interrupt-stack.txt')
        relevant = {pc: old['unresolved'][pc] for pc in ctx}
        raw, names, graphs = saved_records(before, app, relevant, after.parent / 'records' / app, executable)
        for pc, contexts_ in sorted(ctx.items()):
            r = relevant[pc]
            chain = ast.literal_eval(r[3])
            load_source = r[5] if r[5] != 'None' else r[4]
            source = contexts_[0]['source'] + '; ' + re.sub(r'site: (\d+)', lambda m: f'site: {int(m[1]):#010x}', load_source)
            if r[5] == 'None':
                barrier = 'No complete all-path target-load proof; parameter/capture or memory contents remain unknown'
                if pc in raw:
                    text = raw[pc]['value'] + str(raw[pc]['expressions'])
                    if 'EntryRegister' in text:
                        barrier += '; symbolic source includes an entry register'
                    if 'EntryStack' in text:
                        barrier += '; symbolic source includes a stack slot'
                if pc in raw:
                    root = re.search(r'Expression \{ id: (\d+) \}', raw[pc]['value'])
                    if root:
                        fact = raw[pc]['expressions'].get(int(root[1]))
                        if fact:
                            source += '; target ' + raw[pc]['register'] + ' = ' + fact['expression'] + ' at ' + fact['pc']
                if graphs.get(pc):
                    barrier += '; ' + re.sub(r'(start|offset|from): (\d+)', lambda m: f'{m[1]}: {int(m[2]):#010x}', graphs[pc][0])
            elif ('typed', pc) in old['sites']:
                barrier = f"Field type known; {len(old['sites'][('typed', pc)])} audited candidates; local resolver outside gate"
            else:
                load_pc = re.search(r'site: (\d+)', r[5])
                barrier = 'No unique callback field at proven target load ' + (f'{int(load_pc[1]):#010x}' if load_pc else r[5])
            after_barrier = barrier
            if ('typed', pc) not in old['sites'] and ('typed', pc) in new['sites']:
                after_barrier = f"Transported field type; {len(new['sites'][('typed', pc)])} audited candidates; mutable object identity remains unknown"
            elif new['loads'].get(pc) and all(row[3] == 'None' for row in new['loads'][pc]):
                after_barrier += '; no field in the retained base suffix'
            registry.append({'app': app, 'pc': pc, 'function': names.get(pc, chain[-1] if chain else r[2]),
                             'contexts': contexts_, 'source': source, 'callback': callback_class(chain),
                             'before_barrier': barrier, 'after_barrier': after_barrier,
                             'inline_chain': chain, 'common_target': raw.get(pc), 'inspection_graph_barriers': graphs.get(pc, []),
                             'load_fields_after': new['loads'].get(pc, [])})
        proofs = comparisons[app] = []
        for key, targets in sorted(new['sites'].items()):
            if key in old['sites']:
                continue
            resolver, pc = key
            assert resolver == 'typed'
            called = {fields(row[3])[0] for row in new['loads'][pc] if fields(row[3])}
            assert called, 'a new site needs an observed field witness'
            excluded = []
            for target, uses in sorted(new['uses'].items()):
                if target in targets:
                    continue
                assert uses and all(u['evidence'].startswith('typed:') and u['fields'] for u in uses), target
                for use in uses:
                    for storage in use['fields']:
                        for field in called:
                            assert new['matches'][(field, tuple(storage))] is False, (pc, target, field, storage)
                excluded.append({'target': target, 'all_uses': uses,
                                 'proof': 'Every address use has nonempty typed storage; the common audit rejects every stored/called field pair'})
            retained = []
            for target in sorted(targets):
                uses = new['uses'].get(target, [])
                witness = next((u for u in uses if not u['evidence'].startswith('typed:') or not u['fields']), None)
                if witness is not None:
                    retained.append({'target': target, 'reason': 'Unknown address use', 'witness': witness})
                elif not uses:
                    retained.append({'target': target, 'reason': 'No address-use evidence'})
                else:
                    matching = [(u, field, storage) for u in uses for field in called for storage in u['fields']
                                if new['matches'][(field, tuple(storage))]]
                    assert matching, (pc, target)
                    use, field, storage = matching[0]
                    retained.append({'target': target, 'reason': 'Compatible audited storage', 'witness': use,
                                     'called_field': field, 'stored_field': storage})
            proofs.append({'site': pc, 'fields': sorted(called), 'targets': sorted(targets), 'retentions': retained,
                           'load_witnesses': new['loads'][pc], 'exclusions': excluded})
        # Bootstrap observations and every previous candidate set are unchanged.
        a = diagnostic(before / f'{app}-bootstrap-callbacks.tsv')
        b = diagnostic(after / f'{app}-bootstrap-callbacks.tsv')
        assert a['uses'] == b['uses'] and a['sites'] == b['sites']
    (output / 'registry.json').write_text(json.dumps(registry, indent=2) + '\n')
    (output / 'candidate-proofs.json').write_text(json.dumps(comparisons, indent=2) + '\n')
    with (output / 'registry.tsv').open('w') as out:
        writer = csv.writer(out, delimiter='\t', lineterminator='\n')
        writer.writerow(['ELF', 'PC', 'Function', 'Hart/level/executor', 'Pointer source', 'Callback class', 'Before barrier', 'After barrier'])
        for row in registry:
            context = '; '.join(f"h{c['hart']}/P{c['level']}/{c['executor']}" for c in row['contexts'])
            writer.writerow([row['app'], row['pc'], row['function'], context, row['source'], row['callback'], row['before_barrier'], row['after_barrier']])
    print(json.dumps({'irq_sites': {app: sum(r['app'] == app for r in registry) for app in ('w0', 'w1')},
                      'new_fields': {app: len(proofs) for app, proofs in comparisons.items()},
                      'excluded_at_new_fields': {app: [len(p['exclusions']) for p in proofs] for app, proofs in comparisons.items()}}, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--records-tool', type=Path, default=Path('target/tools/examples/function-records'))
    args = parser.parse_args()
    main(args.before, args.after, args.output, args.records_tool)
