from lib import *
import time,subprocess
def doc(d):
    return (b'<testsuites>'+b'<testsuite name="s">'*d+b'<testcase classname="c" name="n%d"><failure message="m">x</failure></testcase>'%d+b'</testsuite>'*d+b'</testsuites>')
def psql(db,s):
    return subprocess.run(['docker','exec','-i','sp2pass-pg','psql','-U','fr','-d',db,'-At'],input=s,capture_output=True,text=True).stdout.strip()
for d in [100,500,900,950,960,965,968,970,971,972,973,975,980,985,989,990,991,992,994,995,996,1000,1100,2000,100000]:
    b=doc(d)
    u=f'/api/ingest?repo=d/p{d}&commit_sha=x{d}&ci_run_id=D{d}'
    o=[]
    for c,db in ((pc,'py'),(rc,'rs')):
        try:
            r=c.post(u,headers=A,content=b); x=short(r); 
        except Exception as e: x=('EXC',type(e).__name__,)
        st=''
        if isinstance(x[1],dict) and 'report_id' in x[1]:
            for _ in range(40):
                st=psql(db,f"select status||'|'||retry_count||'|'||coalesce(left(error,50),'') from reports where id={x[1]['report_id']}")
                if not st.startswith('pending'): break
                time.sleep(.1)
            st=st+' cases='+psql(db,f"select count(*) from test_cases tc join projects p on p.id=tc.project_id join repos r on r.id=p.repo_id where r.name='d/p{d}'")
        o.append((x[0], (x[1].get('detail') if isinstance(x[1],dict) else str(x[1]))and str(x[1].get('detail') if isinstance(x[1],dict) else x[1])[:60], st))
    print(d,'PY',o[0],'\n    RS',o[1])
