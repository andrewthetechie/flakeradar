import sys,json,time; sys.path.insert(0,sys.argv[1])
from lib import *
S=sys.argv[2]
X={**A,'Content-Type':'application/xml'}
def up(cases,**q):
    body=f'<testsuite name="ss">{"".join(cases)}</testsuite>'.encode()
    call('POST','/api/ingest',params=q,content=body,headers=X)
def tc(n,st,msg='',file='',line=None,details=''):
    a=f'name="{n}" classname="pk.m"'
    if file: a+=f' file="{file}"'
    if line is not None: a+=f' line="{line}"'
    if st=='p': return f'<testcase {a}/>'
    m=f' message="{msg}"' if msg else ''
    return f'<testcase {a}><failure{m}>{details}</failure></testcase>'
R='gh/one'
# proven flakes + sample-failure cases
for i in range(5):
    sha=f'cafe{i:036d}'
    up([tc('t_py','f','boom',file='tests/a.py',line=3,details='trace'),
        tc('t_ts','f','',file='./src/x.test.ts'),
        tc('t_nofile','f','m2'),
        tc('t_go','f','m3',file='a_test.go',line=0),
        tc('t_java','f','m3',file='A.java'),
        tc('t_rb','f','m3',file='a.rb'),
        tc('t_other','f','m3',file='a.cs'),
        tc('t_noext','f','m3',file='Makefile'),
        tc('t_empty_new','f' if i<4 else 'f','boom older' if i<4 else '')],
        repo=R,project='backend',root='sub/dir' if i else None,commit_sha=sha,branch='main',ci_run_id=str(i),ci_job_id=f'J{i}')
    up([tc(n,'p') for n in ['t_py','t_ts','t_nofile','t_go','t_java','t_rb','t_other','t_noext','t_empty_new']],repo=R,project='backend',commit_sha=sha,branch='main',ci_run_id=str(i),ci_run_attempt=2)
    call('POST','/api/ingest/pipeline',json={'repo':R,'provider':'github','pipeline':'ci.yml','commit_sha':sha,'branch':'main','ci_run_id':str(i),'jobs':[{'ci_job_id':f'JJ{i}','name':'build','status':'failed','url':'https://ci/x','runner_name':'r1','runner_labels':['a']},{'ci_job_id':f'JK{i}','name':'nobody','status':'failed','runner_labels':['l1','l2']}]},headers=A)
    call('POST','/api/ingest/pipeline',json={'repo':R,'provider':'github','pipeline':'ci.yml','commit_sha':sha,'branch':'main','ci_run_id':str(i),'ci_run_attempt':2,'jobs':[{'ci_job_id':f'JJ{i}b','name':'build','status':'passed','url':'','runner_name':'r1'},{'ci_job_id':f'JK{i}b','name':'nobody','status':'passed'}]},headers=A)
    drain(); time.sleep(0.5)
time.sleep(2)
def show():
    a=[json.loads(l) for l in open(S+'/run/gh-py.jsonl')]; b=[json.loads(l) for l in open(S+'/run/gh-rs.jsonl')]
    print('requests',len(a),len(b))
    return a,b
a,b=show()
ka=lambda r:(r['m'],r['p'],json.dumps(r['body'],sort_keys=True))
import difflib
sa=sorted(map(ka,a)); sb=sorted(map(ka,b))
only_a=[x for x in sa if x not in sb]; only_b=[x for x in sb if x not in sa]
print('only py',len(only_a),'only rs',len(only_b))
TSR=re.compile(r'\d{4}-\d\d-\d\d \d\d:\d\d')
for x,y in zip(only_a,only_b):
    bx=TSR.sub('<ts>',x[2]); by=TSR.sub('<ts>',y[2])
    if bx!=by or x[1]!=y[1]:
        print('--- PY',x[0],x[1]); print('--- RS',y[0],y[1])
        for l in difflib.unified_diff(json.loads(x[2]).get('body','').split('\n'),json.loads(y[2]).get('body','').split('\n'),lineterm='',n=0): print('   ',l[:200])
print('seq same order:',[ (r['m'],r['p']) for r in a]==[(r['m'],r['p']) for r in b])
print('hdr',a[0]['h'],'\n   ',b[0]['h'])
for tb in ['test_cases','jobs']:
    x,y=sql(f"select id,name,github_issue_number,github_issue_filed,github_issue_closed,github_issue_fixed_noted_at is not null,github_issue_checked_at is not null,clean_streak,flakiness_score from {tb} order by id")
    print(tb,'same' if x.stdout==y.stdout else ('DIFF',x.stdout,y.stdout))
print(x.stdout[:800])
report()
