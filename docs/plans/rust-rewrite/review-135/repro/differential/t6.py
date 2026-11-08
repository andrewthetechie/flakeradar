import sys,json,time; sys.path.insert(0,sys.argv[1])
from lib import *
X={**A,'Content-Type':'application/xml'}
N=int(sys.argv[2]); DELAY=float(sys.argv[3])
names=[f't{i:03d}' for i in range(N)]
def body(st):
    return ('<testsuite name="ss">'+''.join(f'<testcase name="{n}" classname="c">'+('<failure message="x"/>' if st=='f' else '')+'</testcase>' for n in names)+'</testsuite>').encode()
for c in (pc,rc):
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c1'},content=body('f'),headers=X)
    c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c1'},content=body('p'),headers=X)
drain(); time.sleep(4)
print('linked',[sql("select count(*) from test_cases where github_issue_number is not null")[i].stdout.strip() for i in range(2)])
httpx.post('http://127.0.0.1:18091/__cfg',json={'delay':DELAY}); httpx.post('http://127.0.0.1:18092/__cfg',json={'delay':DELAY})
time.sleep(18)  # a sync is under way on both
start=time.time()
ids={}
for c,lbl in ((pc,'py'),(rc,'rs')):
    ids[lbl]=c.post('/api/ingest',params={'repo':'lk/a','commit_sha':'c2'},content=body('p'),headers=X).json()['report_id']
done={}
qt={}
for lbl,c in (('py',pc),('rs',rc)):
    t=time.time()
    try:
        r=c.post('/api/tests/1/quarantine',json={'quarantined':True,'reason':'x'},timeout=60); qt[lbl]=(r.status_code,round(time.time()-t,1))
    except Exception as e: qt[lbl]=(repr(e),round(time.time()-t,1))
print('quarantine during sync',qt)
while len(done)<2 and time.time()-start<float(sys.argv[4]):
    for lbl,c in (('py',pc),('rs',rc)):
        if lbl in done: continue
        r=c.get(f'/api/reports/{ids[lbl]}').json()
        if r['status']!='pending':
            done[lbl]=(r['status'],round(time.time()-start,1),r['retry_count'],(r['error'] or '')[:300])
    time.sleep(0.5)
print('done',done,'elapsed',round(time.time()-start,1))
for lbl,c in (('py',pc),('rs',rc)):
    print(lbl,c.get(f'/api/reports/{ids[lbl]}').json().get('status'),c.get(f'/api/reports/{ids[lbl]}').json().get('retry_count'),(c.get(f'/api/reports/{ids[lbl]}').json().get('error') or '')[:400])
