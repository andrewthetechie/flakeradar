import re,json
def load(f):
    d={}
    for l in open(f):
        p=l.rstrip('\n').split('|',2)
        if len(p)<3: continue
        b=p[2]
        if b.startswith('\\x'):
            b=bytes.fromhex(b[2:]).decode()
        d.setdefault(p[0],(p[1],b))
    return d
a,b=load('body.py.txt'),load('body.rs.txt')
for k in sorted(set(a)|set(b)):
    if k not in a or k not in b: print('ONLY',k,'py' if k in a else 'rs'); continue
    try:
        ja,jb=json.loads(a[k][1]),json.loads(b[k][1])
        jja,jjb=ja['jobs'][0],jb['jobs'][0]
        print(k,'same' if jja.get('started_at')==jjb.get('started_at') and jja.get('summary')==jjb.get('summary') else 'DIFF',a[k][0],b[k][0],jja.get('started_at'),jjb.get('started_at'),jja.get('summary'),jjb.get('summary'))
    except Exception as e: print(k,'ERR',e,a[k][1][:100],b[k][1][:100])
