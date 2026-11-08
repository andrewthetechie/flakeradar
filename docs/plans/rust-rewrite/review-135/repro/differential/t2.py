import sys,copy; sys.path.insert(0,sys.argv[1])
from lib import *
base={"repo":"o/p","provider":"github","pipeline":".github/workflows/ci.yml","commit_sha":"a1","branch":"main","ci_run_id":"1","ci_run_attempt":1,"jobs":[{"ci_job_id":"j1","name":"build","status":"failed"}]}
def mut(**k):
    b=copy.deepcopy(base); b.update(k); return b
def job(**k):
    b=copy.deepcopy(base); b['jobs'][0].update(k); return b
n=0
def go(p,label):
    global n; n+=1
    p=copy.deepcopy(p); p['repo']=p.get('repo') if p.get('repo')!='o/p' else f'o/p{n}'
    call('POST','/api/ingest/pipeline',json=p,headers=A,label=label)
for lbl,p in [('attempt str',mut(ci_run_attempt="2")),('attempt float',mut(ci_run_attempt=2.0)),('attempt 2.5',mut(ci_run_attempt=2.5)),('attempt true',mut(ci_run_attempt=True)),('attempt null',mut(ci_run_attempt=None)),('attempt 0',mut(ci_run_attempt=0)),('attempt big',mut(ci_run_attempt=3000000000)),('attempt vbig',mut(ci_run_attempt=10**30)),
 ('run_id int',mut(ci_run_id=123)),('run_id null',mut(ci_run_id=None)),('default null',mut(default_branch=None)),('default ws',mut(default_branch="  ")),('default num',mut(default_branch=1)),('branch ws',mut(branch="  ")),('sha ws',mut(commit_sha=" x ")),('sha long',mut(commit_sha="x"*65)),('pipeline long',mut(pipeline="x"*513)),('repo upper',mut(repo=" O/P ")),('repo bad',mut(repo="bad")),('repo long',mut(repo="a/"+"b"*300)),('provider upper',mut(provider=" GitHub ")),('provider bad',mut(provider="git hub")),('provider long',mut(provider="x"*40)),('jobs empty',mut(jobs=[])),('jobs obj',mut(jobs={})),('jobs 1001',mut(jobs=[{"ci_job_id":str(i),"name":"n","status":"passed"} for i in range(1001)])),('extra',mut(extra=1)),
 ('dup ids',mut(jobs=[{"ci_job_id":"a","name":"n","status":"passed"},{"ci_job_id":"a","name":"m","status":"passed"}])),('dup after strip',mut(jobs=[{"ci_job_id":"a","name":"n","status":"passed"},{"ci_job_id":" a ","name":"m","status":"passed"}])),('two errors',mut(commit_sha="",branch="",jobs=[])),('two job errors',mut(jobs=[{"ci_job_id":"","name":"","status":"bogus"}])),
 ('job id int',job(ci_job_id=5)),('job status upper',job(status="FAILED")),('job status cancelled',job(status="cancelled")),('job status null',job(status=None)),('job url null',job(url=None)),('job runner null',job(runner_name=None)),('labels null',job(runner_labels=None)),('labels str',job(runner_labels="x")),('labels int',job(runner_labels=[1])),('labels 51',job(runner_labels=["x"]*51)),('label long',job(runner_labels=["x"*256])),('labels ws',job(runner_labels=[" x "])),
 ('summary str',job(summary="true")),('summary yes',job(summary="yes")),('summary int',job(summary=1)),('summary null',job(summary=None)),('summary 2',job(summary=2)),
 ('started z',job(started_at="2026-01-01T00:00:00Z")),('started offset',job(started_at="2026-01-01T02:00:00+02:00")),('started naive',job(started_at="2026-01-01T00:00:00")),('started space',job(started_at="2026-01-01 00:00:00")),('started date',job(started_at="2026-01-01")),('started epoch',job(started_at=1767225600)),('started epoch str',job(started_at="1767225600")),('started float',job(started_at=1767225600.5)),('started empty',job(started_at="")),('started frac',job(started_at="2026-01-01T00:00:00.123456789Z")),('started lower',job(started_at="2026-01-01t00:00:00z")),('started null',job(started_at=None)),('started bad',job(started_at="yesterday")),('completed ms',job(completed_at="2026-01-01T00:00:00.5+00:00")),('started tz minutes',job(started_at="2026-01-01T00:00+00:00")),('started no sec',job(started_at="2026-01-01T00:00")),('started gitlab',job(started_at="2026-01-01T00:00:00.000Z")),('started gitlab2',job(started_at="2026-01-01T00:00:00.000+00:00")),('started bigepoch',job(started_at=1767225600000)),
 ]:
    go(p,lbl)
drain()
for rep in ['o/p%d'%i for i in range(1,8)]:
    pass
report()
