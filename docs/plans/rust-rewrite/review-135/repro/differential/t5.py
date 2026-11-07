import sys,json,time; sys.path.insert(0,sys.argv[1])
from lib import *
S=sys.argv[2]
X={**A,'Content-Type':'application/xml'}
def up(cases,**q):
    body=f'<testsuite name="ss">{"".join(cases)}</testsuite>'.encode()
    call('POST','/api/ingest',params=q,content=body,headers=X)
def tc(n,st,msg='',details=''):
    a=f'name="{n}" classname="pk.m"'
    if st=='p': return f'<testcase {a}/>'
    m=f' message="{msg}"' if msg else ''
    return f'<testcase {a}><failure{m}>{details}</failure></testcase>'
def cfg(**k):
    httpx.post('http://127.0.0.1:18091/__cfg',json=k); httpx.post('http://127.0.0.1:18092/__cfg',json=k)
def logs():
    return [json.loads(l) for l in open(S+'/run/gh-py.jsonl')],[json.loads(l) for l in open(S+'/run/gh-rs.jsonl')]
TSR=re.compile(r'\d{4}-\d\d-\d\d \d\d:\d\d')
mark=[0,0]
def compare(label):
    a,b=logs(); na=a[mark[0]:]; nb=b[mark[1]:]; mark[0]=len(a); mark[1]=len(b)
    k=lambda r:(r['m'],r['p'],TSR.sub('<ts>',json.dumps(r['body'],sort_keys=True,ensure_ascii=False)))
    ka=[k(r) for r in na]; kb=[k(r) for r in nb]
    print(f'[{label}] py={len(ka)} rs={len(kb)} same={sorted(ka)==sorted(kb)}')
    if sorted(ka)!=sorted(kb):
        for x in ka:
            if x not in kb: print('  only PY',str(x)[:1200])
        for x in kb:
            if x not in ka: print('  only RS',str(x)[:1200])
def dbcmp(label):
    for tb in ['test_cases','jobs']:
        x,y=sql(f"select id,name,github_issue_number,github_issue_filed,github_issue_closed,github_issue_fixed_noted_at is not null,clean_streak,flakiness_score from {tb} order by id")
        print(f'[{label}] {tb}','same' if x.stdout==y.stdout else ('DIFF\nPY '+x.stdout+'\nRS '+y.stdout))
R='gh/two'
time.sleep(1)
# 1. sample failure with empty newest failing execution
up([tc('t_sample','f','boom older','older details')],repo=R,commit_sha='s1',branch='main'); drain()
up([tc('t_sample','f')],repo=R,commit_sha='s2',branch='main'); drain()
up([tc('t_sample','p')],repo=R,commit_sha='s2',branch='main'); drain(); time.sleep(1)
compare('sample-failure'); dbcmp('sample')
# 2. proven flakes for two more repos, then fixed note and close
for r,n in [('gh/three','t_a'),('gh/three','t_b'),('gh/four','t_c')]:
    up([tc(n,'f','x')],repo=r,commit_sha='c1',branch='main'); up([tc(n,'p')],repo=r,commit_sha='c1',branch='main')
drain(); time.sleep(1); compare('filing 3'); dbcmp('filed')
for i in range(2,5):
    for r,n in [('gh/three','t_a'),('gh/three','t_b'),('gh/four','t_c')]:
        up([tc(n,'p')],repo=r,commit_sha=f'c{i}',branch='main')
    drain(); time.sleep(1)
compare('fixed note/close'); dbcmp('noted')
# 3. sync re-arm/keep: issue numbers
time.sleep(5)
x,y=sql("select name,github_issue_number from test_cases where github_issue_number is not null order by id"); print('links',x.stdout.replace('\n',' '),'|',y.stdout.replace('\n',' '))
report()
