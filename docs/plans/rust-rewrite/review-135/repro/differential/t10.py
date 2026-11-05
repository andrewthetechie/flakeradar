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
drain(); time.sleep(2)
# Network failure in sync: drop every GET
for p in (18091,18092): httpx.post(f'http://127.0.0.1:{p}/__cfg',json={'drop':['GET /repos']})
n0=[sum(1 for l in open(S+f'/run/gh-{x}.jsonl') if '"m": "GET"' in l) for x in('py','rs')]
time.sleep(8)
n1=[sum(1 for l in open(S+f'/run/gh-{x}.jsonl') if '"m": "GET"' in l) for x in('py','rs')]
print('GETs in one sync with GitHub unreachable: py',n1[0]-n0[0],'rs',n1[1]-n0[1])
for p in (18091,18092): httpx.post(f'http://127.0.0.1:{p}/__cfg',json={'drop':[], 'delay':2.0})
time.sleep(9)  # slow sync under way on both
def pidof(pat):
    return int(subprocess.run(['pgrep','-f',pat],capture_output=True,text=True).stdout.split()[0])
for lbl,pat in (('py','uvicorn gh_main:app'),('rs','target/debug/flakeradar-patched')):
    pid=pidof(pat); t=time.time(); os.kill(pid,signal.SIGTERM)
    while True:
        try: os.kill(pid,0)
        except ProcessLookupError: break
        if time.time()-t>100: print(lbl,'still alive after 100s'); os.kill(pid,signal.SIGKILL); break
        time.sleep(0.2)
    print(lbl,'exit after SIGTERM',round(time.time()-t,1),'s')
