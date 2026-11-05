import sys,json,gzip; sys.path.insert(0,sys.argv[1])
from lib import *
xml=b'<testsuite name="s"><testcase name="t" classname="c"/></testsuite>'
Q={'repo':'m/p','commit_sha':'a1'}
n=[0]
def q():
    n[0]+=1
    return {'repo':'m/p','commit_sha':f'a{n[0]}'}
def mp(parts,boundary='BOUND',close=True,ct=None,preamble=b''):
    b=preamble
    for hdr,val in parts:
        b+=b'--'+boundary.encode()+b'\r\n'+hdr+b'\r\n\r\n'+val+b'\r\n'
    if close: b+=b'--'+boundary.encode()+b'--\r\n'
    return b,(ct or f'multipart/form-data; boundary={boundary}')
F=b'Content-Disposition: form-data; name="report"; filename="r.xml"'
cases={
 'plain':mp([(F,xml)]),
 'with ct':mp([(F+b'\r\nContent-Type: application/xml',xml)]),
 'no filename':mp([(b'Content-Disposition: form-data; name="report"',xml)]),
 'empty filename':mp([(b'Content-Disposition: form-data; name="report"; filename=""',xml)]),
 'filename star':mp([(b"Content-Disposition: form-data; name=\"report\"; filename*=UTF-8''r.xml",xml)]),
 'wrong field':mp([(b'Content-Disposition: form-data; name="file"; filename="r.xml"',xml)]),
 'two fields first bad':mp([(b'Content-Disposition: form-data; name="report"; filename="a.xml"',b'not xml'),(F,xml)]),
 'two fields last bad':mp([(F,xml),(b'Content-Disposition: form-data; name="report"; filename="a.xml"',b'not xml')]),
 'text then file':mp([(b'Content-Disposition: form-data; name="report"',b'x'),(F,xml)]),
 'file then text':mp([(F,xml),(b'Content-Disposition: form-data; name="report"',b'x')]),
 'other then report':mp([(b'Content-Disposition: form-data; name="a"',b'x'),(F,xml)]),
 'quoted boundary':mp([(F,xml)],ct='multipart/form-data; boundary="BOUND"'),
 'no boundary':mp([(F,xml)],ct='multipart/form-data'),
 'boundary upper param':mp([(F,xml)],ct='multipart/form-data; BOUNDARY=BOUND'),
 'wrong boundary':mp([(F,xml)],ct='multipart/form-data; boundary=OTHER'),
 'ct charset':mp([(F,xml)],ct='multipart/form-data; charset=utf-8; boundary=BOUND'),
 'ct upper':mp([(F,xml)],ct='Multipart/Form-Data; boundary=BOUND'),
 'unclosed':mp([(F,xml)],close=False),
 'preamble crlf':mp([(F,xml)],preamble=b'\r\n'),
 'preamble text':mp([(F,xml)],preamble=b'hello\r\n'),
 'lf only':(b'--BOUND\n'+F+b'\n\n'+xml+b'\n--BOUND--\n','multipart/form-data; boundary=BOUND'),
 'empty file':mp([(F,b'')]),
 'header case':mp([(F.replace(b'Content-Disposition',b'CONTENT-DISPOSITION'),xml)]),
 'unquoted name':mp([(b'Content-Disposition: form-data; name=report; filename=r.xml',xml)]),
 'single quoted':mp([(b"Content-Disposition: form-data; name='report'; filename='r.xml'",xml)]),
 'filename first':mp([(b'Content-Disposition: form-data; filename="r.xml"; name="report"',xml)]),
 'non form-data disp':mp([(b'Content-Disposition: attachment; name="report"; filename="r.xml"',xml)]),
 'no disposition':mp([(b'Content-Type: text/plain',xml)]),
 'empty body':(b'','multipart/form-data; boundary=BOUND'),
 'name with semicolon':mp([(b'Content-Disposition: form-data; name="x;name=report"; filename="r.xml"',xml)]),
 'body has boundary text':mp([(F,xml+b'\n--BOUNDX')]),
 'trailing whitespace boundary':(b'--BOUND  \r\n'+F+b'\r\n\r\n'+xml+b'\r\n--BOUND--\r\n','multipart/form-data; boundary=BOUND'),
 'transfer enc base64':mp([(F+b'\r\nContent-Transfer-Encoding: base64',__import__('base64').b64encode(xml))]),
 'header folding':mp([(b'Content-Disposition: form-data;\r\n name="report"; filename="r.xml"',xml)]),
 'bad header line':mp([(b'garbage\r\n'+F,xml)]),
}
for lbl,(b,ct) in cases.items():
    call('POST','/api/ingest',params=q(),content=b,headers={**A,'Content-Type':ct},label='MP '+lbl)
for lbl,b,h in [('raw xml',xml,{'Content-Type':'application/xml'}),('urlenc',xml,{'Content-Type':'application/x-www-form-urlencoded'}),('no ct',xml,{}),('json ct',xml,{'Content-Type':'application/json'}),('gzip body + header',gzip.compress(xml),{'Content-Type':'application/xml','Content-Encoding':'gzip'}),('gzip body no header',gzip.compress(xml),{'Content-Type':'application/xml'}),('empty',b'',{'Content-Type':'application/xml'}),('ws',b' \n',{'Content-Type':'application/xml'}),]:
    call('POST','/api/ingest',params=q(),content=b,headers={**A,**h},label='RAW '+lbl)
# order of errors
call('POST','/api/ingest',params={'repo':'bad','commit_sha':'x'},content=b'',headers=A,label='empty+badrepo')
call('POST','/api/ingest',params={'repo':'bad','commit_sha':'x'},content=b'junk',headers=A,label='junk+badrepo')
call('POST','/api/ingest',params={'repo':'o/r','project':'Bad Name!','commit_sha':'x'},content=b'junk',headers=A,label='badproject')
call('POST','/api/ingest',params={'repo':'o/r','root':'../x','commit_sha':'x'},content=xml,headers=A,label='badroot')
call('POST','/api/ingest',params={'repo':'o/r','root':' /abs/ ','commit_sha':'x'},content=xml,headers=A,label='abs root')
call('POST','/api/ingest',params={'repo':'O/R ','commit_sha':'x','branch':'','ci_run_id':' x '},content=xml,headers=A,label='upper repo')
call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'dup','default_branch':'  ','ci_job_id':'  ','pipeline':'   '},content=xml,headers=A,label='blanks')
call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'dup','default_branch':'','ci_job_id':'','pipeline':''},content=xml,headers=A,label='blanks dup')
call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'dup2','line_base':'0'},content=xml,headers=A,label='lb0')
call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'dup2','line_base':'1'},content=xml,headers=A,label='lb1')
call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'dup2','line_base':'1'},content=xml,headers=A,label='lb1 dup')
for la in ['0','2','-1','x','','1.0']:
    call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'z','line_base':la},content=xml,headers=A,label='linebase '+la)
for a in ['0','-1','x','','1.0',' 1','3000000000','99999999999999999999999']:
    call('POST','/api/ingest',params={'repo':'o/r','commit_sha':'z','ci_run_attempt':a},content=xml,headers=A,label='attempt '+a)
# size limits
big=b'<testsuite><testcase name="t"/></testsuite><!--'+b'x'*(20*1024*1024)+b'-->'
call('POST','/api/ingest',params=Q,content=big[:20*1024*1024],headers=A,label='exact20 (invalid xml)')
call('POST','/api/ingest',params=Q,content=big[:20*1024*1024+1],headers=A,label='20+1')
call('POST','/api/ingest',params=Q,content=big[:20*1024*1024+70000],headers=A,label='20+70k')
huge=b'{"a":"'+b'x'*(21*1024*1024)+b'"}'
call('POST','/api/ingest/pipeline',content=huge,headers={**A,'Content-Type':'application/json'},label='pipeline 21MB json')
call('POST','/api/tests/1/quarantine',content=huge,headers={'Content-Type':'application/json'},label='quarantine 21MB json')
call('POST','/api/ingest',params=Q,content=xml,headers={'X-API-Key':'wrong'},label='badkey')
call('POST','/api/ingest',params={},content=xml,headers={'X-API-Key':'wrong'},label='badkey+noquery')
call('POST','/api/ingest',params={},content=xml,headers=A,label='noquery')
call('POST','/api/ingest',params=Q,content=xml,headers={'x-api-key':TOK+' '},label='key trailing ws')
call('GET','/api/quarantine',headers={'X-API-Key':''},label='empty key')
call('GET','/api/quarantine',label='no key')
report()
