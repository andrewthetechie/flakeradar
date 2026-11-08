from lib import *
import time,subprocess
L=20*1024*1024
XML=b'<testsuite name="s"><testcase classname="c" name="n"/></testsuite>'
def raw(sz): return XML+b' '*(sz-len(XML))
def gen(b,chunk=1<<16):
    for i in range(0,len(b),chunk): yield b[i:i+chunk]
n=[50]
def bothg(label,u,h,body):
    out=[]
    for c in (pc,rc):
        n[0]+=1
        try: out.append(short(c.post(u+f'&ci_run_id=C{n[0]}' if '?' in u else u,headers=h,content=gen(body))))
        except Exception as e: out.append(('EXC',type(e).__name__))
    print(('same ' if [o[0] for o in out]==[o[0]for o in [out[0],out[0]]] else 'DIFF ')+label,out[0],'|',out[1])
for lab,sz in [('L',L),('L+1',L+1),('L+65536',L+65536),('L+65537',L+65537),('L+1M',L+(1<<20))]:
    bothg('chunked raw '+lab,'/api/ingest?repo=l/c&commit_sha=c2',A,raw(sz))
J={"ci_job_id":"1","name":"j","status":"passed"}
jb=json.dumps({"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"limc","branch":"m","ci_run_id":"JC","jobs":[J]}).encode()
JH={**A,'content-type':'application/json'}
for lab,sz in [('L',L),('L+1',L+1)]:
    bothg('chunked json '+lab,'/api/ingest/pipeline',JH,jb+b' '*(sz-len(jb)))
print('### staged 40001')
def psql(db,s): return subprocess.run(['docker','exec','sp2pass-pg','psql','-U','fr','-d',db,'-Atc',s],capture_output=True,text=True)
ALWAYS="""create or replace function sp_fail() returns trigger language plpgsql as $f$ begin if nextval('sp_seq') <= %d then raise exception 'staged' using errcode='40001'; end if; return new; end $f$;"""
for label,failn in [('always',10**6),('once',1)]:
  for db,c in (('py',pc),('rs',rc)):
    psql(db,"drop trigger if exists sp_t on runs; drop sequence if exists sp_seq; create sequence sp_seq;"+ALWAYS%failn+"create trigger sp_t before insert on runs for each row execute function sp_fail();")
    r=c.post(f'/api/ingest?repo=ser/x&commit_sha={label}&ci_run_id=SER{label}',headers=A,content=XML.replace(b'name="n"',b'name="%s"'%label.encode()))
    rid=r.json()['report_id'];t=time.time()
    while time.time()-t<25:
        st=psql(db,f"select status,retry_count,coalesce(left(error,70),''),run_id is not null from reports where id={rid}").stdout.strip()
        if not st.startswith('pending'): break
        time.sleep(.2)
    print(db,label,st,'after',round(time.time()-t,1),'s; seq',psql(db,"select last_value from sp_seq").stdout.strip(),'tests',psql(db,f"select count(*) from tests where name='{label}'").stdout.strip(),'runs',psql(db,f"select count(*) from runs where ci_run_id='SER{label}'").stdout.strip())
    psql(db,"drop trigger if exists sp_t on runs")
