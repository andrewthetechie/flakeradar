from lib import *
import subprocess,time
J={"ci_job_id":"1","name":"j","status":"passed"}
base={"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"abc","branch":"m","jobs":[J]}
H={**A,'content-type':'application/json'}
vals=[2e10,20000000001,-2e10,-20000000001,1767225600000,1767225600000.5,1767225600123,1e14,1e15,1e16,1e17,1e20,253402300799,253402300800,"1767225600000","1767225600","1767225600.5","-5","0","1e3"," 1767225600","1_000"]
for i,v in enumerate(vals):
    both(f'started_at={v!r}','POST','/api/ingest/pipeline',content=json.dumps({**base,"ci_run_id":f"u{i}","jobs":[{**J,"started_at":v}]}).encode(),headers=H)
time.sleep(1.5)
for db in ('py','rs'):
    print(db,subprocess.run(['docker','exec','sp2pass-pg','psql','-U','fr','-d',db,'-Atc',"select je.ci_run_id, je.started_at from job_executions je where je.ci_run_id like 'u%' order by length(je.ci_run_id), je.ci_run_id"],capture_output=True,text=True).stdout)
print("### other lax::integer callers")
for c in (pc,rc):
    r=c.post('/api/ingest?repo=o/r&commit_sha=seed',headers=A,content=b'<testsuite name="s"><testcase classname="c" name="n"><failure message="m">d</failure></testcase></testsuite>'); print(r.status_code)
time.sleep(1.5)
tid=[c.get('/api/tests',params={'include_stable':'true'}).json()['items'][0]['id'] for c in (pc,rc)]; print('tid',tid)
def per(label,suffix,m='POST',**kw):
    out=[]
    for c,t in zip((pc,rc),tid):
        try: out.append(short(c.request(m,f'/api/tests/{t}{suffix}',**kw)))
        except Exception as e: out.append(('EXC',type(e).__name__))
    print(('same ' if out[0]==out[1] else 'DIFF ')+label,json.dumps(out[0])[:300], '' if out[0]==out[1] else '\n   RS '+json.dumps(out[1])[:300])
for v in ['1e19','1e20','9223372036854775807','"7"','7.0','7.5','0','-1','1e3','true','100000000']:
    per('quarantine exp='+v,'/quarantine',content=('{"expires_in_days":%s}'%v).encode(),headers=H)
    per('unq','/quarantine',m='DELETE',headers=A) if False else None
for v in ['1e19','1e20','"7"','7.0','true','2147483648','9223372036854775807']:
    per('link number='+v,'/issue',content=('{"number":%s}'%v).encode(),headers=H)
    per('link2 number='+v,'/github-issue',content=('{"number":%s}'%v).encode(),headers=H)
