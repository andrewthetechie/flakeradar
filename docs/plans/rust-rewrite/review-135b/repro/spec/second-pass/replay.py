import httpx,json,re,time,sys,subprocess
ROOT='/Users/andrew/Documents/code/flakeradar/tests/parity'
TS=re.compile(r'^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?(Z|\+00:00)?$')
def norm(v):
    if isinstance(v,dict): return {k:norm(x) for k,x in v.items()}
    if isinstance(v,list): return [norm(x) for x in v]
    if isinstance(v,str) and TS.match(v): return '<timestamp>'
    return v
def replay(name,port,db):
    sc=json.load(open(f'{ROOT}/scenarios/{name}.json')); c=httpx.Client(base_url=f'http://127.0.0.1:{port}',timeout=30,follow_redirects=False)
    ok=bad=0
    for i,st in enumerate(sc['steps'],1):
        g=json.load(open(f'{ROOT}/golden/{name}/{i:02d}.json'))
        if 'http' in st:
            h=st['http']; hd={k:v.replace('$TOKEN','tok-spec-review') for k,v in h.get('headers',{}).items()}
            kw={}
            if 'body_text' in h: kw['content']=h['body_text'].encode()
            elif 'json' in h: kw['json']=h['json']
            elif 'body_file' in h: kw['content']=open(f"{ROOT}/{h['body_file']}",'rb').read()
            elif 'body' in h: kw['json']=h['body']
            r=c.request(h['method'],h['path'],params=h.get('query'),headers=hd,**kw)
            try: b=r.json()
            except Exception: b=r.text
            m=(r.status_code==g['status'] and norm(b)==norm(g['body']))
            if not m: print('MISMATCH',name,i,'status',r.status_code,g['status'],'\n  got ',json.dumps(norm(b))[:500],'\n  gold',json.dumps(norm(g['body']))[:500])
        elif st.get('wait_processed'):
            for _ in range(300):
                s=c.get('/api/reports/summary').json()
                if s['pending']==0: break
                time.sleep(.05)
            m=(s.get('failed')==g.get('reports',{}).get('failed') and g.get('processed') is True)
            print('wait',json.dumps(g)[:150],'got',s)
        elif 'dump_db' in st:
            out={}
            for t in st['dump_db']:
                t2=t if isinstance(t,str) else t.get('table')
                rows=subprocess.run(['docker','exec','sp2pass-pg','psql','-U','fr','-d',db,'-Atc',f"select coalesce(json_agg(x order by x.id),'[]') from {t2} x"],capture_output=True,text=True).stdout
                out[t2]=norm(json.loads(rows))
            m=norm(g['tables'])==out
            if not m: print('DUMP MISMATCH',json.dumps(out)[:400],json.dumps(g['tables'])[:400])
        else: print('unknown step',st); m=False
        ok+=m; bad+=(not m)
    print(name,'on reference: match',ok,'mismatch',bad)
replay('failed-report',18281,'pyf'); replay('github',18282,'pyg')
