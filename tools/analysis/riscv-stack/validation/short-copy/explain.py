#!/usr/bin/env python3
"""Validate a real contextual call against canonical records and generic effects."""
import argparse
from collections import Counter, deque
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROM_SHA256 = 'a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87'
CALL, TRAMPOLINE, BODY = 0x5004db66, 0x2f8004c4, 0x2f80d260
GUARD, REGION_START, REGION_END = 0x2f80d298, 0x2f80d2d8, 0x2f80d332


def rows(text):
    return [line.split('\t') for line in text.splitlines()]


def top_level_items(text):
    result, start, level = [], 0, 0
    for i, char in enumerate(text):
        if char in '[{(': level += 1
        elif char in ']})': level -= 1
        elif char == ',' and level == 0:
            result.append(text[start:i].strip())
            start = i + 1
    result.append(text[start:].strip())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('rom', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--before-records', required=True, type=Path)
    parser.add_argument('--before-effects', required=True, type=Path)
    parser.add_argument('--examples', type=Path, default=Path('target/tools/examples'))
    args = parser.parse_args()
    assert hashlib.sha256(args.rom.read_bytes()).hexdigest() == ROM_SHA256
    records_text = subprocess.check_output([str(args.examples/'function-records'),str(args.rom),f'{BODY:x}'],text=True)
    effects_text = subprocess.check_output([str(args.examples/'stack-call-writes'),str(args.elf),str(args.rom),'chips/esp32s31/platform/rom-functions.toml',f'{CALL:x}','-600'],text=True)
    records, effects = rows(records_text), rows(effects_text)
    before = rows(args.before_effects.read_text())
    # The full generic observations and every old uncertainty remain identical.
    assert records_text == args.before_records.read_text()
    for kind in ('rom-write','rom-barrier','call-write','call-barrier','inputs','target','preserves'):
        assert [r for r in effects if r[0]==kind] == [r for r in before if r[0]==kind], kind
    target = next(r[2] for r in effects if r[:2]==['target',f'{CALL:#010x}'])
    assert target == f'Some(ImageAddress {{ address: {TRAMPOLINE} }})'
    inputs = next(r[2] for r in effects if r[:2]==['inputs',f'{CALL:#010x}'])
    inputs = top_level_items(inputs.removeprefix('Some([').removesuffix('])'))
    assert len(inputs)==32 and inputs[10]=='Unknown' and inputs[12]=='Constant { value: 20 }'
    assert any(r[0]=='rom-transfer' and r[1]==f'{TRAMPOLINE:#010x}' and r[2]=='call=false' and r[3]==f'ImageAddress {{ address: {BODY} }}' for r in effects)
    context = next(r[1] for r in effects if r[0]=='context')
    assert f'entry: {BODY}' in context
    assert 'arguments: [None, None, Some(20), None, None, None, None, None]' in context
    branch_rows = [r[2] for r in effects if r[:2]==['context-branch',f'{BODY:#010x}']]
    branches = {}
    for row in branch_rows:
        match = re.fullmatch(r'BranchProof \{ site: (\d+), test: (\w+), left: \[([^]]+)\], left_count: (\d+), right: \[([^]]+)\], right_count: (\d+), taken: (true|false) \}',row)
        assert match, row
        pc,test,left,nleft,right,nright,taken = match.groups()
        branches[int(pc)] = {'test':test,'left':list(map(int,left.split(', ')))[:int(nleft)],'right':list(map(int,right.split(', ')))[:int(nright)],'taken':taken=='true'}
    assert set(branches)=={BODY+6,GUARD}
    assert branches[BODY+6] == {'test':'Ltu','left':[3],'right':[20],'taken':True}
    assert branches[GUARD] == {'test':'Lt','left':[32],'right':[16,20],'taken':False}
    instructions = {int(r[1],16):r[2] for r in records if r[0]=='instruction'}
    for pc, instruction in {
        BODY:'li a5, 3',BODY+2:'add a3, a0, a2',BODY+6:'bltu a5, a2, 30',0x2f80d28c:'andi a4, a3, -4',
        0x2f80d290:'sub a6, a4, a5',0x2f80d294:'li a2, 32',GUARD:'blt a2, a6, 144',
    }.items(): assert instructions[pc]==instruction
    aligned = [r[1] for r in records if r[0]=='aligned-value' and f'point: Entry {{ offset: {0x2f80d28c} }}, register: 15,' in r[1]]
    assert len(aligned)==1 and 'origin: AllIncoming' in aligned[0]
    expressions = {int(r[1]):r[3] for r in records if r[0]=='expression'}
    initial_id = int(re.search(r'initial: Expression \{ id: (\d+) \}',aligned[0])[1])
    value_id = int(re.search(r', value: Expression \{ id: (\d+) \}',aligned[0])[1])
    assert expressions[initial_id]=='Some(EntryRegister { register: 10 })'
    mask_node = re.fullmatch(r'Some\(Integer \{ op: And, left: Expression \{ id: (\d+) \}, right: Constant \{ value: 4294967292 \} \}\)',expressions[value_id])
    assert mask_node
    sum_id=int(mask_node[1])
    assert expressions[sum_id]==f'Some(Integer {{ op: Add, left: Expression {{ id: {initial_id} }}, right: Constant {{ value: 3 }} }})'
    entry_length_id = next(id_ for id_,text in expressions.items() if text=='Some(EntryRegister { register: 12 })')
    end_id = int(re.fullmatch(r'Expression \{ id: (\d+) \}',next(r[3] for r in records if r[:3]==['register-write',f'{BODY+2:#010x}','13']))[1])
    assert expressions[end_id]==f'Some(Integer {{ op: Add, left: Expression {{ id: {initial_id} }}, right: Expression {{ id: {entry_length_id} }} }})'
    expression_definitions={f'e{id_}':expressions[id_] for id_ in (initial_id,entry_length_id,end_id,sum_id,value_id)}
    # Independently audit the common graph with only the certified branch cuts.
    edges = []
    for row in records:
        if row[0]=='graph' and (m:=re.fullmatch(r'Edge \{ from: (\d+), target: Some\((\d+)\), relation: (\w+), external: false \}',row[1])):
            source,target,kind=m.groups()
            edges.append((int(source),int(target),kind))
    region = {pc for pc in instructions if REGION_START <= pc < REGION_END}
    boundary = [e for e in edges if e[0] not in region and e[1] in region]
    assert boundary == [(GUARD,0x2f80d328,'Taken')],boundary
    incoming_head = [e for e in edges if e[1]==REGION_START]
    assert incoming_head == [(0x2f80d32c,REGION_START,'Taken')]
    reached, queue = {BODY}, deque([BODY])
    while queue:
        source = queue.popleft()
        for src,target,kind in edges:
            if src!=source: continue
            branch=branches.get(src)
            if branch and ((kind=='Taken' and not branch['taken']) or (kind=='Fallthrough' and branch['taken'])): continue
            if target not in reached: reached.add(target); queue.append(target)
    assert not reached & region
    excluded = {int(r[2],16) for r in effects if r[:2]==['context-unreachable',f'{BODY:#010x}']}
    assert region <= excluded and not reached & excluded
    stores = {int(r[1],16):int(r[3]) for r in records if r[0]=='memory' and r[2]=='Store'}
    generic = Counter(int(r[1],16) for r in effects if r[:1]==['rom-write'] and r[2]==f'{BODY:#010x}')
    applied = Counter(int(r[1],16) for r in effects if r[0]=='context-call-write')
    bulk = {pc for pc in stores if pc in region}
    assert len(stores)==12 and len(bulk)==9 and sum(generic.values())==14
    assert not bulk & applied.keys() and sum(applied.values())==5
    assert applied == Counter({0x2f80d27a:1,0x2f80d2c8:3,0x2f80d338:1})
    assert all(stores[pc]==4 for pc in bulk)
    for pc in applied:
        old = [r[2] for r in effects if r[0]=='call-write' and int(r[1],16)==pc]
        new = [r[2] for r in effects if r[0]=='context-call-write' and int(r[1],16)==pc]
        assert old==new
    unknown_sites = {int(r[1],16) for r in effects if r[0]=='context-call-barrier' and 'kind: UnknownStore' in r[2]}
    assert unknown_sites==set(applied)
    assert ['context-preserves','false'] in effects
    # All residues and RV32 wrapping words; no non-overflow premise.
    residues = []
    for r in range(4):
        p = (r+3)&0xfffffffc
        e = (r+20)&0xfffffffc
        residual = (e-p)&0xffffffff
        assert residual in (16,20)
        for base in (0,0x7ffffffc,0xfffffff0,0xfffffffc):
            d=base+r
            assert ((((d+20)&0xffffffff)&0xfffffffc)-(((d+3)&0xffffffff)&0xfffffffc))&0xffffffff == residual
        residues.append({'D_mod_4':r,'P_minus_D':p-r,'R':residual})
    table = [{'pc':f'{pc:#010x}','width':width,'generic_effects':generic[pc],
              'generic':'three finite May byte addresses; unknown contents' if pc==0x2f80d2c8 else 'UnknownStore',
              'call_effects':applied[pc], 'call':'excluded: unreachable' if pc in bulk else ('three May byte effects; destination unknown at caller' if pc==0x2f80d2c8 else 'UnknownStore retained')}
             for pc,width in sorted(stores.items())]
    proof = {'rom_sha256':ROM_SHA256,'call':f'{CALL:#x}','trampoline':f'{TRAMPOLINE:#x}','body':f'{BODY:#x}',
             'entry_a0':'unknown','entry_a2':20,'context':context,'alignment_fact':aligned[0],'expression_definitions':expression_definitions,
             'residues':residues,'bulk_guard':branches[GUARD],
             'all_region_boundary_entries':[[hex(a),hex(b),k] for a,b,k in boundary],
             'head_entries':[[hex(a),hex(b),k] for a,b,k in incoming_head],
             'unreachable_region_instructions':len(region),'generic_effects':sum(generic.values()),
             'contextual_effects':sum(applied.values()),'store_sites':table,'complete_preservation':False}
    args.output.mkdir(parents=True,exist_ok=True)
    (args.output/'records.tsv').write_text(records_text)
    (args.output/'facts.tsv').write_text(effects_text)
    (args.output/'proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    print(json.dumps({'stores':len(stores),'excluded_bulk_stores':len(bulk),'generic_effects':14,'contextual_effects':5}))

if __name__=='__main__': main()
