import httpx, json, sys
PY='http://127.0.0.1:18181'; RS='http://127.0.0.1:18182'; A={'X-API-Key':'tok-spec-review'}
pc=httpx.Client(base_url=PY,timeout=120); rc=httpx.Client(base_url=RS,timeout=120)
def short(r):
    try: b=r.json()
    except Exception: return (r.status_code, r.text[:160])
    if isinstance(b,dict) and isinstance(b.get('detail'),list):
        return (r.status_code,[(i.get('type'),i.get('loc'),i.get('msg')) for i in b['detail']])
    if isinstance(b,dict) and 'items' in b: return (r.status_code,{k:v for k,v in b.items() if k!='items'})
    return (r.status_code,b)
def both(label,m,u,**kw):
    out=[]
    for c in (pc,rc):
        try: out.append(short(c.request(m,u,**kw)))
        except Exception as e: out.append(('EXC',type(e).__name__))
    same = out[0]==out[1]
    print(('same ' if same else 'DIFF ')+label); 
    if same: print('   ',json.dumps(out[0],ensure_ascii=True)[:170])
    else: print('  PY',json.dumps(out[0],ensure_ascii=True)[:600]); print('  RS',json.dumps(out[1],ensure_ascii=True)[:600])
    return out
