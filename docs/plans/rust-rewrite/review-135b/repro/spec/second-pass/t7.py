from lib import *
import time,subprocess
XML=b'<testsuite name="s"><testcase classname="c" name="n"/></testsuite>'
def psql(db,s):
    r=subprocess.run(['docker','exec','-i','sp2pass-pg','psql','-U','fr','-d',db,'-At'],input=s,capture_output=True,text=True)
    return r.stdout.strip()+(' ERR '+r.stderr.strip()[:120] if 'ERROR' in r.stderr else '')
F="""drop trigger if exists sp_t on test_runs; drop sequence if exists sp_seq; create sequence sp_seq;
create or replace function sp_fail() returns trigger language plpgsql as $f$ begin if nextval('sp_seq') <= %d then raise exception 'staged' using errcode='40001'; end if; return new; end $f$;
create trigger sp_t before insert on test_runs for each row execute function sp_fail();"""
for label,failn in [('alwaysB',10**6),('onceB',1),('twiceB',2),('quadB',4)]:
  for db,c in (('py',pc),('rs',rc)):
    o=psql(db,F%failn); print("trigger", "ERR" if "ERROR" in o else "ok", end=" ")
    r=c.post(f'/api/ingest?repo=ser/x&commit_sha={label}&ci_run_id=SER{label}',headers=A,content=XML.replace(b'name="n"',b'name="%s"'%label.encode()))
    rid=r.json()['report_id'];t=time.time();st=''
    while time.time()-t<40:
        st=psql(db,f"select status,retry_count,coalesce(left(error,60),''),run_id is not null from reports where id={rid}")
        if not st.startswith('pending'): break
        time.sleep(.2)
    print(db,label,st,'after',round(time.time()-t,1),'s; fired',psql(db,"select last_value from sp_seq"),'rows-named',psql(db,f"select count(*) from test_cases where name='{label}'"))
    psql(db,"drop trigger if exists sp_t on test_runs")
