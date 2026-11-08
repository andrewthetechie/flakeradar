import sys,json,time,subprocess,os,signal; sys.path.insert(0,sys.argv[1])
from lib import *
S=sys.argv[2]
X={**A,'Content-Type':'application/xml'}
names=[f't{i:03d}' for i in range(30)]
def body(st,names=names):
    return ('<testsuite name="ss">'+''.join(f'<testcase name="{n}" classname="c">'+('<failure message="x"/>' if st=='f' else '')+'</testcase>' for n in names)+'</testsuite>').encode()
for c in (pc,rc):
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c1'},content=body('f'),headers=X)
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c1'},content=body('p'),headers=X)
drain(); time.sleep(3)
print('linked',[sql("select count(*) from test_cases where github_issue_number is not null")[i].stdout.strip() for i in range(2)])
# Part 1: Postgres restart: leadership drops and returns. Then a new flaky Test must still be filed.
n0=[sum(1 for _ in open(S+f'/run/gh-{x}.jsonl')) for x in('py','rs')]
subprocess.run(['docker','restart','pr135-spec-pg'],capture_output=True)
time.sleep(10)
for lbl,c in (('py',pc),('rs',rc)):
    for _ in range(60):
        try:
            r=c.get('/api/health/ready').json()
            if r.get('processor_leader'): break
        except Exception as e: r=repr(e)
        time.sleep(1)
    print('after restart',lbl,r)
for c in (pc,rc):
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c9'},content=body('f',['new1']),headers=X)
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c9'},content=body('p',['new1']),headers=X)
drain(); time.sleep(12)
for x in ('py','rs'):
    rows=[json.loads(l) for l in open(S+f'/run/gh-{x}.jsonl')]
    posts=[r for r in rows if r['m']=='POST' and r['p'].endswith('/issues')]
    gets=[r for r in rows if r['m']=='GET']
    print(x,'POST issues total',len(posts),'last title',posts[-1]['body']['title'] if posts else None,'GET total',len(gets),'last GET age',round(time.time()-gets[-1]['t'],1) if gets else None)
print('issue number on new1:',[sql("select github_issue_number from test_cases where name='new1'")[i].stdout.strip() for i in range(2)])
