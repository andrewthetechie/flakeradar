import sys; sys.path.insert(0,sys.argv[1])
from lib import *
xml=b'<testsuite name="s"><testcase name="t" classname="c"/></testsuite>'
Q={'repo':'o/r','commit_sha':'a1'}
X={**A,'Content-Type':'application/xml'}
# methods / framework errors
for m,u in [('POST','/api/tests'),('PUT','/api/health'),('DELETE','/api/repos'),('HEAD','/api/health'),('HEAD','/api/tests'),('HEAD','/metrics'),('HEAD','/openapi.json'),('OPTIONS','/api/tests'),('OPTIONS','/api/health'),('GET','/api/nope'),('POST','/api/nope'),('GET','/nope'),('POST','/nope'),('GET','/'),('HEAD','/'),('GET','/docs'),('GET','/redoc'),('GET','/mcp'),('GET','/mcp/'),('DELETE','/mcp/'),('PUT','/mcp/'),('GET','/api/'),('GET','/api'),('PATCH','/api/tests/1/issue'),('GET','/api/ingest'),('PUT','/api/ingest'),('GET','/api/tests/1'),('GET','/api/jobs/1'),('GET','/api/reports/1/retry'),('POST','/api/health/'),('POST','/api/ingest/'),('GET','/metrics/'),('GET','//api/health'),('GET','/api//health'),('GET','/api/tests//'),('GET','/%2Fapi'),('GET','/api/tests/1/history/'),('DELETE','/api/repos/a/b/'),('GET','/api/repos?x=1')]:
    call(m,u,headers=A if m in('POST','PUT','DELETE','PATCH') else None)
# CORS
for origin in ['http://localhost:5173','http://evil.example']:
    call('GET','/api/tests',params={'repo':'o/r'},headers={'Origin':origin})
    call('OPTIONS','/api/tests',headers={'Origin':origin,'Access-Control-Request-Method':'GET'})
    call('OPTIONS','/api/ingest',headers={'Origin':origin,'Access-Control-Request-Method':'POST','Access-Control-Request-Headers':'X-API-Key, Content-Type'})
    call('OPTIONS','/api/nope',headers={'Origin':origin,'Access-Control-Request-Method':'DELETE'})
# query parsing
for u in ['/api/reports','/api/tests','/api/jobs','/api/quarantine']:
    for lim in ['10.0','+5',' 5','5 ','1_0','0x10','1e1','true','','٣','-0','05','10.5','2147483648','9223372036854775808','99999999999999999999']:
        h=A if u=='/api/quarantine' else None
        call('GET',u,params={'limit':lim},headers=h)
call('GET','/api/reports?limit=1&limit=2')
call('GET','/api/reports?limit=x&limit=2')
call('GET','/api/reports?limit=2&limit=x')
call('GET','/api/reports?status=pending&status=bogus')
call('GET','/api/reports?unknown=1')
call('GET','/api/reports?limit')
call('GET','/api/reports?limit=%zz')
call('GET','/api/reports?%zz=1')
call('GET','/api/reports?status=')
call('GET','/api/reports?status=PENDING')
call('GET','/api/tests?include_stable=yes&repo=o/r')
for v in ['1','0','true','false','True','FALSE','on','off','yes','no','y','n','t','f','','2','maybe',' true ','1.0']:
    call('GET','/api/tests',params={'include_stable':v})
    call('GET','/api/tests',params={'flaky_only':v})
for v in ['-1','0','1','91','90','abc','']:
    call('GET','/api/tests',params={'new_within_days':v})
for v in ['-1','0','abc','']:
    call('GET','/api/tests',params={'offset':v})
for v in ['score','last_seen','proven','bogus','','SCORE']:
    call('GET','/api/tests',params={'sort':v}); call('GET','/api/jobs',params={'sort':v})
for v in ['network','environment','timing','assertion','other','bogus','']:
    call('GET','/api/tests',params={'category':v})
call('GET','/api/tests',params={'repo':'','project':'','q':''})
call('GET','/api/tests',params={'q':'x'*1000})
call('GET','/api/jobs',params={'repo':'','pipeline':'','q':''})
call('GET','/api/summary',params={'repo':''}); call('GET','/api/summary',params={'repo':'nope/nope'}); call('GET','/api/summary',params={'project':'x'})
call('GET','/api/jobs/summary',params={'repo':''}); call('GET','/api/jobs/summary',params={'repo':'nope/nope','window':'x'})
# path params
for p in ['0','-1','1','99999999999','9223372036854775808','1.0','+1','%201','١','1_0','abc','']:
    call('GET',f'/api/tests/{p}/history'); call('GET',f'/api/jobs/{p}/history'); call('GET',f'/api/reports/{p}')
    call('POST',f'/api/tests/{p}/quarantine',json={'quarantined':True})
    call('PUT',f'/api/tests/{p}/issue',json={'number':1},headers=A)
    call('DELETE',f'/api/tests/{p}/issue',headers=A)
    call('DELETE',f'/api/reports/{p}',headers=A); call('POST',f'/api/reports/{p}/retry',headers=A)
# bodies
for b,ct in [(b'{',"application/json"),(b'',"application/json"),(b'[]',"application/json"),(b'{"quarantined":true}',"text/plain"),(b'{"quarantined":true}',None),(b'{"quarantined":"yes"}',"application/json"),(b'{"quarantined":1}',"application/json"),(b'{"quarantined":null}',"application/json"),(b'{}',"application/json"),(b'{"quarantined":true,"reason":123}',"application/json"),(b'{"quarantined":true,"expires_in_days":"5"}',"application/json"),(b'{"quarantined":true,"expires_in_days":5.0}',"application/json"),(b'{"quarantined":true,"expires_in_days":5.5}',"application/json"),(b'{"quarantined":true,"expires_in_days":0}',"application/json"),(b'{"quarantined":true,"expires_in_days":366}',"application/json"),(b'{"quarantined":true,"extra":1}',"application/json"),(b'{"quarantined":true,"quarantined":false}',"application/json"),(b'\xff',"application/json"),(b'{"quarantined":true}',"application/json; charset=utf-8"),(b'{"quarantined":true}',"application/vnd.api+json"),(b'{"quarantined":true}',"APPLICATION/JSON"),(b'{"quarantined":true}',"application/jsonx")]:
    h={'Content-Type':ct} if ct else {}
    call('POST','/api/tests/999/quarantine',content=b,headers=h,label=f'quarantine body {b!r} ct={ct}')
    call('PUT','/api/tests/999/issue',content=b.replace(b'quarantined',b'number'),headers={**h,**A},label=f'issue body {b!r} ct={ct}')
    call('POST','/api/ingest/pipeline',content=b,headers={**h,**A},label=f'pipeline body {b!r} ct={ct}')
report()
