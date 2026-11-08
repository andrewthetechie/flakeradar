from lib import *
import time,subprocess
def psql(db,s): return subprocess.run(['docker','exec','-i','sp2pass-pg','psql','-U','fr','-d',db,'-At'],input=s,capture_output=True,text=True).stdout.strip()
def doc(d): return b'<testsuite name="s">'*d+b'<testcase classname="c" name="n"/>'+b'</testsuite>'*d
for d in [988,989,990,991,992,993,994,995]:
    o=[]
    for c,db in ((pc,'py'),(rc,'rs')):
        for a in range(2):
            try: r=c.post(f'/api/ingest?repo=b/p{d}&commit_sha=x&ci_run_id=B',headers=A,content=doc(d)); break
            except Exception: continue
        j=short(r); st=''
        if isinstance(j[1],dict) and 'report_id' in j[1]:
            time.sleep(.3); st=psql(db,f"select status||'|'||coalesce(left(error,40),'') from reports where id={j[1]['report_id']}")
        o.append((j[0],st,str(j[1])[:50] if j[0]!=202 else ''))
    print(d,'PY',o[0],'RS',o[1])
