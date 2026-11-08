import httpx, json, re, sys, time, subprocess
PY='http://127.0.0.1:18081'; RS='http://127.0.0.1:18082'
TOK='tok-spec-review'
A={'X-API-Key':TOK}
TS=re.compile(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?(Z|\+00:00)?')
pc=httpx.Client(base_url=PY,timeout=60,follow_redirects=False); rc=httpx.Client(base_url=RS,timeout=60,follow_redirects=False)
def norm(v):
    if isinstance(v,dict): return {k:norm(x) for k,x in v.items()}
    if isinstance(v,list): return [norm(x) for x in v]
    if isinstance(v,str): return TS.sub('<ts>',v)
    if isinstance(v,float) and v==int(v): return int(v)
    return v
def strip422(b):
    if isinstance(b,dict) and isinstance(b.get('detail'),list):
        return {'detail':[{'type':i.get('type'),'loc':i.get('loc')} for i in b['detail']]}
    return b
def body(r):
    try: return r.json()
    except Exception: return r.text
diffs=[]
same=0
def call(method,url,label=None,headers_of=('content-type','location','allow','www-authenticate','cache-control','access-control-allow-origin','access-control-allow-methods','access-control-allow-headers','access-control-allow-credentials','vary','access-control-max-age'),**kw):
    global same
    def rq(c):
        for i in range(3):
            try: return c.request(method,url,**kw)
            except httpx.TransportError as e:
                last=e; time.sleep(0.3)
        class R: status_code=0; text=repr(last); headers={}
        R.json=lambda self=None: (_ for _ in ()).throw(ValueError())
        return R()
    r1=rq(pc); r2=rq(rc)
    b1,b2=strip422(norm(body(r1))),strip422(norm(body(r2)))
    h1={h:r1.headers.get(h).lower() for h in headers_of if r1.headers.get(h) is not None}; h2={h:r2.headers.get(h).lower() for h in headers_of if r2.headers.get(h) is not None}
    for h in (h1,h2):
        if 'location' in h: h['location']=re.sub(r'^http://127\.0\.0\.1:\d+','',h['location'])
    if r1.status_code!=r2.status_code or b1!=b2 or h1!=h2:
        diffs.append((label or (method+' '+url+' '+str({k:v for k,v in kw.items() if k=='params'})),r1.status_code,r2.status_code,b1,b2,h1,h2))
        print('DIFF',label or (method,url,kw.get('params')),r1.status_code,r2.status_code,'\n  PY',json.dumps(b1,ensure_ascii=True)[:500],h1,'\n  RS',json.dumps(b2,ensure_ascii=True)[:500],h2)
    else: same+=1
    return r1,r2
def drain():
    for c in (pc,rc):
        for _ in range(600):
            if c.get('/api/reports/summary').json()['pending']==0: break
            time.sleep(0.05)
def sql(s,db=('py','rs')):
    out=[]
    for d in db:
        out.append(subprocess.run(['docker','exec','pr135-spec-pg','psql','-U','fr','-d',d,'-Atc',s],capture_output=True,text=True))
    return out
def report():
    print('same',same,'diffs',len(diffs))
