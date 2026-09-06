#!/usr/bin/env python3
"""Explicit whitelist import. Does not modify the source checkout or fetch data."""
import argparse, hashlib, json, pathlib, re, subprocess, urllib.parse
ROOT = pathlib.Path(__file__).resolve().parents[3]
p = argparse.ArgumentParser(); p.add_argument('--source', required=True); args = p.parse_args()
source = pathlib.Path(args.source).resolve()
revision = subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'], text=True).strip()
dirty = subprocess.check_output(['git','-C',str(source),'status','--porcelain','--untracked-files=all','--','data/sites'], text=True)
if dirty.strip():
    raise ValueError('source data/sites has uncommitted changes; a pinned source revision is required')
assets = ROOT/'assets/search'
host_groups = {}
ptd_source = (ROOT/'src/ptd_sites.rs').read_text()
for hosts, identifier in re.findall(r'((?:\s*"[^"]+"\s*\|?)+)\s*=>\s*\{?\s*Some\("([^"]+)"\)', ptd_source):
    host_groups[identifier] = re.findall(r'"([^"]+)"',hosts)
def host(url):
    name = urllib.parse.urlsplit(url).hostname
    return name.encode('idna').decode().lower().rstrip('.') if name else None
raw = [json.loads(path.read_text()) for path in sorted((source/'data/sites').glob('*.json'))]
# Materialize explicit namespace pairs only from cited PTD definition filenames
# or exact canonical host evidence. Identical spellings alone prove no identity.
crosswalk = {}
for s in raw:
    for citation in s.get('sources', []):
        url = citation.get('url', '')
        match = re.fullmatch(r'https://(?:github\.com/pt-plugins/PT-depiler/blob|raw\.githubusercontent\.com/pt-plugins/PT-depiler)/[0-9a-f]+/src/packages/site/definitions/([^/]+)\.ts', url)
        if match and match[1] in host_groups:
            ptd_id = match[1]
            if ptd_id in crosswalk and crosswalk[ptd_id] != s['id']:
                raise ValueError(f'ambiguous cited PTD identity {ptd_id}')
            crosswalk[ptd_id] = s['id']
for s in raw:
    canonical = host(s.get('url',''))
    for ptd_id, hosts in host_groups.items():
        if canonical in hosts:
            if ptd_id in crosswalk and crosswalk[ptd_id] != s['id']:
                raise ValueError(f'ambiguous PTD identity {ptd_id}')
            crosswalk[ptd_id] = s['id']
rows=[]
for s in raw:
    ptd_ids=sorted(k for k,v in crosswalk.items() if v==s['id'])
    hosts=sorted(set([h for h in [host(s.get('url',''))] if h]+[h for p in ptd_ids for h in host_groups[p]]))
    aka=list(dict.fromkeys(s.get('aka',[])))
    row=dict(id=s['id'],name=s['name'],aka=aka,url=s.get('url',''),hosts=hosts,ptd_ids=ptd_ids,
        pt_circle=s.get('pt_circle','unknown'),categories=s.get('resource_categories',[]),
        content_types=s.get('content_types',[]),specialties=s.get('specialties',[]),
        official_groups=s.get('official_groups',[]),community_labels=s.get('community_labels',[]),
        description=s.get('features','')[:800],beginner_tips=s.get('beginner_tips','')[:500],
        sources=s.get('sources',[]),checked_at=s.get('checked_at',''))
    # Names and mutable signup status are not the subject of semantic embeddings.
    row['resource_text']='; '.join(row['categories']+row['content_types'])
    row['feature_text']='; '.join(row['specialties']+row['community_labels']+[row['description'],row['beginner_tips']])
    rows.append(row)
rows.sort(key=lambda s:s['id'])
for name,data in [('catalog.json',{'source_revision':revision,'sites':rows}),('ptd-crosswalk.json',dict(sorted(crosswalk.items()))),('overrides.json',{'version':1,'host_source_revision':'e9fae952f8200ed06a0822baae6f6f6ae84b2f5a','aliases':{},'pronunciations':{'重庆':'chongqing','重邮':'chongyou','音乐':'yinyue','快乐':'kuaile'}})]:
    (assets/name).write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
print(f'Imported {len(rows)} public sites at {revision}; {len(crosswalk)} explicit PTD mappings')
