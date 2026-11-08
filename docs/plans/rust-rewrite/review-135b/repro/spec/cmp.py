import json,sys
p={};r={}
for l in open(sys.argv[1]): d=json.loads(l);p[d['id']]=d['r']
for l in open(sys.argv[2]): d=json.loads(l);r[d['id']]=d['r']
def cls(x):
    if 'cases' in x: return 'OK'
    if 'crash' in x: return 'CRASH'
    if 'time_error' in x: return 'TIME400'
    return 'ERR'
n=0;bad=0
for k in p:
    n+=1
    a,b=p[k],r.get(k,{'error':'MISSING'})
    if cls(a)=='OK' and cls(b)=='OK' and a==b: continue
    if cls(a)==cls(b)=='ERR': continue
    bad+=1
    def sh(x):
        if 'cases' in x:
            return [(c['suite'],c['classname'],c['name'],c['status'],c['duration'],c['message'][:30],c['details'][:30],c['file'],c['line']) for c in x['cases'][:4]]
        return x
    print(k,'\n   PY',str(sh(a))[:230],'\n   RS',str(sh(b))[:230])
print('total',n,'diff',bad)
