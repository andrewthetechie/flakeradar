from lib import *
import subprocess
J={"ci_job_id":"1","name":"j","status":"passed"}
base={"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"abc","branch":"m","jobs":[J]}
H={**A,'content-type':'application/json'}
print("#### attempt, valid job")
job=json.dumps(J)
tmpl='{"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"abc","branch":"m","ci_run_id":"RID","ci_run_attempt":%s,"jobs":[%s]}'
for i,v in enumerate(['2147483647','2147483648','3e9','9223372036854775807','9223372036854775808','18446744073709551616','1e19','1e20','1e27','1e28','"99999999999999999999"','9007199254740993']):
    both('attempt='+v,'POST','/api/ingest/pipeline',content=(tmpl%(v,job)).replace('RID',f'a{i}').encode(),headers=H)
for db in ('py','rs'):
    print(db,subprocess.run(['docker','exec','sp2pass-pg','psql','-U','fr','-d',db,'-Atc',"select ci_run_id,ci_run_attempt,status,left(coalesce(error,''),70) from reports where ci_run_id like 'a%' order by id"],capture_output=True,text=True).stdout)
print("#### body multi-error")
def bad(label,obj):
    both(label,'POST','/api/ingest/pipeline',content=(obj if isinstance(obj,bytes) else json.dumps(obj).encode()),headers=H)
bad('{}',{})
for raw in [b'[]',b'5',b'null',b'"x"',b'',b'{',b'true']: bad(repr(raw),raw)
bad('all wrong',{"repo":5,"provider":6,"pipeline":7,"commit_sha":8,"branch":9,"default_branch":10,"ci_run_id":11,"ci_run_attempt":"x","jobs":12})
bad('reversed order keys',dict(reversed(list({"repo":5,"provider":6,"pipeline":"","commit_sha":"","branch":"","jobs":[]}.items()))))
bad('empties',{"repo":"","provider":"","pipeline":"  ","commit_sha":" ","branch":"","jobs":[]})
bad('repo bad+rest',{**base,"repo":"noslash","provider":"Weird","commit_sha":""})
bad('too long',{**base,"repo":"a/"+"b"*300,"provider":"p"*40,"pipeline":"p"*600,"commit_sha":"c"*70,"branch":"b"*300,"default_branch":"d"*300,"ci_run_id":"r"*300})
bad('job all wrong',{**base,"jobs":[{"ci_job_id":1,"name":2,"status":3,"url":4,"runner_name":5,"runner_labels":6,"started_at":[],"completed_at":{},"summary":"maybe"}]})
bad('job labels',{**base,"jobs":[{**J,"runner_labels":["ok",5,"x"*300,None]}]})
bad('job labels 51',{**base,"jobs":[{**J,"runner_labels":["l"]*51}]})
bad('job two bad',{**base,"jobs":[{"name":"a"},5,{**J,"status":"x"},J]})
bad('job dup ids',{**base,"jobs":[J,J]})
bad('job not dict',{**base,"jobs":["x"]})
bad('jobs 1001',{**base,"jobs":[J]*1001})
bad('job blank',{**base,"jobs":[{**J,"name":"   ","ci_job_id":"  "}]})
for i,v in enumerate(["2026-01-01","2026-01-01T00:00","2026-01-01 00:00:00Z","20260101T000000Z","2026-13-01","x",0,1e10,1e11,-1,True,None,"1700000000","1700000000.5","2026-01-01T00:00:00+25:00","2026-01-01T24:00:00Z","","  2026-01-01T00:00:00Z ","2026-1-1"]):
    bad(f'started_at={v!r}',{**base,"ci_run_id":f"t{i}","jobs":[{**J,"started_at":v}]})
for v in [1,0,"yes","no","on","off","t","f","y","n","1","0","true","FALSE",2,1.0,0.0,None,""]:
    bad(f'summary={v!r}',{**base,"ci_run_id":f"s{v!r}","jobs":[{**J,"summary":v}]})
print("#### query multi")
Q='/api/ingest?'
both('ingest missing both','POST',Q,headers=A,content=b'<x/>')
both('ingest missing+bad','POST',Q+'line_base=9',headers=A,content=b'<x/>')
both('ingest bad two','POST',Q+'repo=o/r&commit_sha=&line_base=9&ci_run_attempt=0',headers=A,content=b'<x/>')
both('ingest long two','POST',Q+'repo='+'r'*300+'&commit_sha='+'c'*70,headers=A,content=b'<x/>')
both('tests range two','GET','/api/tests?page=0&page_size=0')
both('tests range+type','GET','/api/tests?page=0&page_size=x')
both('tests type+range','GET','/api/tests?page=x&page_size=0')
both('tests bool+int','GET','/api/tests?has_issue=x&new_within_days=x')
both('path+query','GET','/api/tests/abc/history?limit=x')
both('path+query2','GET','/api/tests/1/history?limit=0')
both('reports','GET','/api/reports?limit=0&status=zz')
both('ci-runs','GET','/api/ci-runs/1/failures?ci_run_attempt=0')
