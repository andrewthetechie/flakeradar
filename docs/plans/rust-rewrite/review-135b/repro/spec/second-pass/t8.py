import subprocess,os,sys,json
BIN='/Users/andrew/Documents/code/flakeradar/target/debug/flakeradar'
PYBASE='/tmp/python-base/backend'
PROBE="""
import sys
sys.path.insert(0,'.')
from app.config import Settings,assert_secure_token
try:
    s=Settings(_env_file=None); assert_secure_token(s.api_token); print('PY OK')
except BaseException as e:
    print('PY REFUSED',type(e).__name__, str(e)[:300].replace(chr(10),' '))
"""
CLOSED='postgresql://fr:pw@127.0.0.1:1/x'
def rs(env):
    e={k:v for k,v in os.environ.items() if not k.startswith('FLAKERADAR')}; e.update({'FLAKERADAR_API_TOKEN':'t','FLAKERADAR_DATABASE_URL':CLOSED,'FLAKERADAR_PORT':'18199','RUST_LOG':'warn'}); e.update(env)
    try:
        p=subprocess.run([BIN],env=e,capture_output=True,text=True,timeout=2.5,cwd='/private/tmp'); out=p.stdout+p.stderr; rc=p.returncode
    except subprocess.TimeoutExpired as x:
        out=((x.stdout or b'').decode()+(x.stderr or b'').decode()); rc='timeout'
    return rc,out
def py(env):
    e={k:v for k,v in os.environ.items() if not k.startswith('FLAKERADAR')}; e.update({'FLAKERADAR_API_TOKEN':'t','FLAKERADAR_DATABASE_URL':'postgresql+asyncpg://fr:pw@127.0.0.1:1/x'}); e.update(env)
    p=subprocess.run(['/tmp/pyref/bin/python','-c',PROBE],env=e,capture_output=True,text=True,cwd=PYBASE); return (p.stdout+p.stderr).strip()[:220]
PW='s3cretpw'
print('#### secret leak table: does the value reach output')
for lab,env,sec in [
 ('bad port','postgresql://u:%s@db:notaport/d'%PW,PW),
 ('at in pw','postgresql://u:%s@x@db:notaport/d'%PW,PW),
 ('slash in pw','postgresql://u:%s/x@db:notaport/d'%PW,PW),
 ('hash in pw','postgresql://u:%s#x@db:notaport/d'%PW,PW),
 ('qmark in pw','postgresql://u:%s?x@db:notaport/d'%PW,PW),
 ('no user','postgresql://:%s@db:notaport/d'%PW,PW),
 ('query password','postgresql://db:notaport/d?password=%s'%PW,PW),
 ('query password ok-port','postgresql://u@127.0.0.1:1/d?password=%s'%PW,PW),
 ('bad scheme','sqlite://u:%s@db/x'%PW,PW),
 ('no scheme','u:%s@db:5432/x'%PW,PW),
 ('asyncpg bad','postgresql+asyncpg://u:%s@db:notaport/d'%PW,PW),
 ('bad sslmode','postgresql://u:%s@127.0.0.1:1/d?sslmode=zzz'%PW,PW),
 ('valid unreachable','postgresql://u:%s@127.0.0.1:1/d'%PW,PW)]:
    rc,out=rs({'FLAKERADAR_DATABASE_URL':env}); print(lab,'rc',rc,'LEAK' if sec in out else 'clean','|',out.strip().splitlines()[-1][:190] if out.strip() else '')
print('#### other credential vars in a bad parse')
names=json.loads(subprocess.run(['/tmp/pyref/bin/python','-c',"import sys,json;sys.path.insert(0,'.');from app.config import Settings;print(json.dumps({k:str(v.annotation) for k,v in Settings.model_fields.items()}))"],capture_output=True,text=True,cwd=PYBASE,env={**os.environ,'FLAKERADAR_API_TOKEN':'t'}).stdout)
print(names)
print('#### token forms')
for t in ['changeme',' changeme','changeme ','CHANGEME','','  ']:
    rc,o=rs({'FLAKERADAR_API_TOKEN':t}); print(repr(t),'|',py({'FLAKERADAR_API_TOKEN':t})[:60],'| RS rc',rc,o.strip().splitlines()[-1][:110] if o.strip() else '')
print('#### typed forms')
intf=[k for k,v in names.items() if 'int' in v][:1]; flt=[k for k,v in names.items() if 'float' in v][:1]; bl=[k for k,v in names.items() if 'bool' in v][:2]; 
print(intf,flt,bl)
for k,vals in [(intf[0],['5','+5',' 5 ','5.0','1_000','5.5','1e3','٣','','abc','99999999999999999999']),(flt[0],['0.5',' 0.5 ','1e-1','.5','5e-1','1_0','٠.٥','NaN','','abc','Infinity']),(bl[0] if bl else '',['true','1','yes','on','t','Y','FALSE','0','off','',' true ','maybe'])]:
    if not k: continue
    for v in vals:
        E={'FLAKERADAR_'+k.upper():v}; rc,o=rs(E); pp=py(E); last=o.strip().splitlines()[-1][:120] if o.strip() else ''
        rsok=rc=='timeout' or 'FLAKERADAR_'+k.upper() not in o
        print(k,repr(v),'PY',pp[:9].strip(),'RS','ok' if rsok else 'REFUSED', '' if pp.startswith('PY OK')==rsok else ' <<DIFF '+pp[:100]+' || '+last)
