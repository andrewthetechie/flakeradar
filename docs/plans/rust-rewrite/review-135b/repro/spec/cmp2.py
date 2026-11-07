import json,collections
p={};r={}
for l in open('py2.jsonl'): d=json.loads(l);p[d['id']]=d['r']
for l in open('rs2.jsonl'): d=json.loads(l);r[d['id']]=d['r']
def cls(x): return 'OK' if 'cases' in x else ('CRASH' if 'crash' in x else 'ERR')
agg=collections.defaultdict(lambda:collections.Counter());ex={}
for k,a in p.items():
    b=r[k]
    if k.startswith('xe'):
        print(k,'PY',str(a)[:150],'\n   RS',str(b)[:150]); continue
    _,lab,by=k.split('|')
    if cls(a)=='OK' and cls(b)=='OK':
        if a!=b:
            kind='name-differs'
            agg[lab][kind]+=1; ex.setdefault((lab,kind),(by,a['cases'][0]['name'],b['cases'][0]['name']))
    elif cls(a)!=cls(b):
        kind=f'{cls(a)}->{cls(b)}'
        agg[lab][kind]+=1; ex.setdefault((lab,kind),(by,str(a)[:80],str(b)[:80]))
for lab,c in sorted(agg.items()):
    print(lab,dict(c),[ex[(lab,k)] for k in c][:2])
print(len(agg),'labels differ of',len({k.split('|')[1] for k in p if k.startswith('sw')}))
