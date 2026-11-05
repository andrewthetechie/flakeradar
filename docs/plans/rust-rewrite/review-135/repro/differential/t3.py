import sys,copy,json; sys.path.insert(0,sys.argv[1])
from lib import *
X={**A,'Content-Type':'application/xml'}
def case(name,status='pass',cls='pkg.mod',msg='',file=None,line=None,time='0.1',extra=''):
    a=f'name="{name}" classname="{cls}" time="{time}"'
    if file: a+=f' file="{file}"'
    if line is not None: a+=f' line="{line}"'
    if status=='pass': inner=''
    elif status=='fail': inner=f'<failure message="{msg}">trace for {name}\nline2</failure><system-out>out</system-out>'
    elif status=='err': inner=f'<error message="{msg}">err {name}</error>'
    elif status=='skip': inner=f'<skipped message="{msg}"/>'
    return f'<testcase {a}>{inner}{extra}</testcase>'
def up(cases,suite='suite',**q):
    body=f'<testsuite name="{suite}">{"".join(cases)}</testsuite>'.encode()
    call('POST','/api/ingest',params=q,content=body,headers=X)
R='acme/app'
sha=0
def s():
    global sha; sha+=1; return f'c{sha:04d}'
# flaky on default with alternating; broken; stable; skip-only; retried
for i in range(14):
    c=s()
    up([case('t_flip','pass' if i%2 else 'fail',msg='connection refused',file='./a/b.py',line=3),
        case('t_broken','fail',msg='AssertionError: x'),
        case('t_stable','pass',file='x/y.py'),
        case('t_skip','skip',msg='nope'),
        case('t_timeout','fail' if i%5==0 else 'pass',msg='deadline exceeded'),
        case('t_err','err' if i%3==0 else 'pass',msg='No space left on device'),
       ],repo=R,project='backend',root='backend',commit_sha=c,branch='main' if i%4 else 'feat/x',ci_run_id=str(1000+i),ci_run_attempt=1,ci_job_id=f'J{i}',pipeline='.github/workflows/ci.yml')
    if i%3==0:
        up([case('t_flip','pass'),case('t_same_sha','fail',msg='expected 1 got 2')],repo=R,project='backend',commit_sha=c,branch='main',ci_run_id=str(1000+i),ci_run_attempt=2,ci_job_id=f'J{i}r')
        up([case('t_same_sha','pass')],repo=R,project='backend',commit_sha=c,branch='main',ci_run_id=str(1000+i),ci_run_attempt=2,ci_job_id=f'J{i}r2')
    # second project & repo
    up([case('fe_flip','pass' if i%3 else 'fail',msg='timed out',cls='fe'),case('fe_ok','pass',cls='fe',line=7,file='src/a.ts')],suite='vitest',repo=R,project='frontend',root='frontend',line_base=0,commit_sha=c,branch='main',ci_run_id=str(1000+i),ci_job_id=f'JF{i}',default_branch='main' if i>6 else None)
    up([case('o_x','fail' if i%2 else 'pass',msg='ECONNRESET')],repo='other/repo',commit_sha=c,branch='dev',ci_run_id=str(2000+i))
    # pipeline
    call('POST','/api/ingest/pipeline',json={'repo':R,'provider':'github','pipeline':'.github/workflows/ci.yml','commit_sha':c,'branch':'main' if i%4 else 'feat/x','ci_run_id':str(1000+i),'ci_run_attempt':1,'default_branch':'main' if i>6 else None,
      'jobs':[{'ci_job_id':f'J{i}','name':'test','status':'failed' if i%2==0 else 'passed','url':f'https://ci/{i}','runner_name':'r1','runner_labels':['ubuntu','x'],'started_at':'2026-01-01T00:00:00Z','completed_at':'2026-01-01T00:01:30.5Z'},
              {'ci_job_id':f'JB{i}','name':'infra','status':'failed' if i%3==0 else 'passed','runner_name':f'r{i%2}','started_at':'2026-01-01T00:00:00+02:00'},
              {'ci_job_id':f'JS{i}','name':'all-green','status':'failed' if i%2==0 else 'passed','summary':True},
              {'ci_job_id':f'JK{i}','name':'skipme','status':'skipped'}]},headers=A)
    call('POST','/api/ingest/pipeline',json={'repo':'other/repo','provider':'gitlab','pipeline':'gl','commit_sha':c,'branch':'dev','ci_run_id':str(2000+i),'jobs':[{'ci_job_id':f'GL{i}','name':'g','status':'failed' if i%2 else 'passed','url':'https://gitlab/x'}]},headers=A)
drain()
# duplicate
up([case('o_x','pass')],repo='other/repo',commit_sha='dupe',branch='dev'); up([case('o_x','pass')],repo='other/repo',commit_sha='dupe',branch='dev'); drain()
# time travel: same on both DBs
sql("""insert into test_score_history(test_case_id,day,flakiness_score,confirmed_flake_count,executions,failures) select id,(now() at time zone 'utc')::date - 20,0.1,0,3,1 from test_cases where name in ('t_flip','fe_flip') on conflict do nothing""")
sql("""insert into test_score_history(test_case_id,day,flakiness_score,confirmed_flake_count,executions,failures) select id,(now() at time zone 'utc')::date - 15,0.9,2,3,1 from test_cases where name in ('t_timeout','t_err') on conflict do nothing""")
sql("""insert into job_score_history(job_id,day,flakiness_score,confirmed_flake_count,executions,failures) select id,(now() at time zone 'utc')::date - 20,0.05,0,3,1 from jobs where name in ('test') on conflict do nothing""")
# quarantine, links
tests=pc.get('/api/tests',params={'repo':R,'include_stable':'true','limit':100}).json()
print('total',tests.get('total'))
for t in tests['items'][:3]:
    call('POST',f"/api/tests/{t['id']}/quarantine",json={'quarantined':True,'reason':'because','expires_in_days':5})
call('POST',f"/api/tests/{tests['items'][3]['id']}/quarantine",json={'quarantined':True})
call('PUT',f"/api/tests/{tests['items'][4]['id']}/issue",json={'number':77},headers=A)
jobs=pc.get('/api/jobs',params={'repo':R,'include_stable':'true','limit':100}).json()
call('PUT',f"/api/jobs/{jobs['items'][0]['id']}/issue",json={'number':78},headers=A)
# dumps compare
import subprocess
for tb,o in [('repos','id'),('projects','id'),('test_cases','id'),('test_runs','id'),('test_executions','id'),('pipelines','id'),('jobs','id'),('job_executions','id'),('reports','id'),('test_score_history','test_case_id,day'),('job_score_history','job_id,day')]:
    a,b=sql(f"select coalesce(json_agg(t),'[]') from (select * from {tb} order by {o}) t")
    ja,jb=json.loads(a.stdout),json.loads(b.stdout)
    na,nb=norm(ja),norm(jb)
    if len(ja)!=len(jb): print('DUMP LEN',tb,len(ja),len(jb))
    diffs_=[]
    for x,y in zip(na,nb):
        for k in x:
            if x[k]!=y[k] and k not in('body',):
                diffs_.append((x.get('id'),k,x[k],y[k]))
    print('dump',tb,len(ja),'rowdiffs',len(diffs_),diffs_[:5])
ids=[t['id'] for t in tests['items']]
jids=[j['id'] for j in jobs['items']]
# reads
for rq in [{},{'repo':R},{'repo':R,'project':'backend'},{'repo':R,'project':'frontend'},{'repo':'other/repo'},{'repo':'none/none'}]:
  for extra in [{},{'include_stable':'true'},{'flaky_only':'true'},{'new_within_days':'7'},{'new_within_days':'30'},{'new_within_days':'1'},{'category':'network'},{'category':'timing'},{'quarantined':'true'},{'quarantined':'false'},{'broken':'true'},{'broken':'false'},{'has_issue':'true'},{'has_issue':'false'},{'q':'flip'},{'q':'%'},{'q':'_'},{'q':'t\\'},{'q':'PKG.MOD'},{'sort':'last_seen'},{'sort':'proven'},{'sort':'score'},{'limit':'2'},{'limit':'2','offset':'2'},{'offset':'1000'},{'include_stable':'true','sort':'proven'},{'include_stable':'true','sort':'last_seen'},{'flaky_only':'true','include_stable':'true'},{'quarantined':'true','include_stable':'true'},{'broken':'true','flaky_only':'true'}]:
    call('GET','/api/tests',params={**rq,**extra})
  call('GET','/api/summary',params=rq)
for rq in [{},{'repo':R},{'repo':R,'pipeline':'.github/workflows/ci.yml'},{'repo':'other/repo'},{'repo':R,'pipeline':'nope'},{'pipeline':'x'}]:
  for extra in [{},{'include_stable':'true'},{'flaky_only':'true'},{'new_within_days':'7'},{'q':'t'},{'q':'%'},{'sort':'last_seen'},{'sort':'proven'},{'limit':'1'},{'limit':'1','offset':'1'},{'has_issue':'true'},{'has_issue':'false'},{'provider':'github'},{'include_summary':'true'},{'summary':'true'}]:
    call('GET','/api/jobs',params={**rq,**extra})
  call('GET','/api/jobs/summary',params=rq)
call('GET','/api/repos')
call('GET','/api/quarantine',headers=A); call('GET','/api/quarantine',params={'repo':R},headers=A); call('GET','/api/quarantine',params={'repo':R,'project':'backend'},headers=A);call('GET','/api/quarantine',params={'project':'x'},headers=A)
for i in ids: 
    call('GET',f'/api/tests/{i}/history'); call('GET',f'/api/tests/{i}/history',params={'limit':3}); call('GET',f'/api/tests/{i}/history',params={'limit':0}); call('GET',f'/api/tests/{i}/history',params={'limit':1000})
for i in jids:
    call('GET',f'/api/jobs/{i}/history'); call('GET',f'/api/jobs/{i}/history',params={'limit':2})
for run in [1000,1001,1003,1006,2000,'nope','']:
  for rq in [{'repo':R},{'repo':'other/repo'},{'repo':R,'ci_run_attempt':1},{'repo':R,'ci_run_attempt':2},{'repo':R,'ci_run_attempt':9},{'repo':R,'ci_run_attempt':0},{'repo':R,'limit':1},{},{'repo':'bad'}]:
    call('GET',f'/api/ci-runs/{run}/failures',params=rq)
call('GET','/api/reports',params={'limit':200}); call('GET','/api/reports',params={'status':'processed','limit':3}); call('GET','/api/reports/summary')
call('GET','/metrics')
report()
