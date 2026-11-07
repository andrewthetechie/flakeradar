from lib import *
import socket,time,subprocess
L=20*1024*1024
XML=b'<testsuite name="s"><testcase classname="c" name="n"/></testsuite>'
n=[0]
def U():
    n[0]+=1; return f'/api/ingest?repo=l/p&commit_sha=c1&ci_run_id=L{n[0]}'
def raw(sz): return XML+b' '*(sz-len(XML))
def mpb(sz,extra=b''):
    return extra+b'--BB\r\nContent-Disposition: form-data; name="report"; filename="r"\r\n\r\n'+raw(sz)+b'\r\n--BB--\r\n'
MP={**A,'content-type':'multipart/form-data; boundary=BB'}
for lab,sz in [('L',L),('L+1',L+1),('L+64K',L+65536),('L+64K+1',L+65537),('L+10M',L+10*1024*1024)]:
    both('raw '+lab,'POST',U(),headers=A,content=raw(sz))
for lab,sz in [('L',L),('L+1',L+1),('L+64K-60',L+65536-60),('L+64K',L+65536),('L+1M',L+(1<<20))]:
    both('mp file '+lab,'POST',U(),headers=MP,content=mpb(sz))
other=b'--BB\r\nContent-Disposition: form-data; name="a"\r\n\r\n'+b'v'*100000+b'\r\n'
both('mp file L + 100KB other field','POST',U(),headers=MP,content=mpb(L,other))
both('mp file 1K + 21MiB other FILE','POST',U(),headers=MP,content=b'--BB\r\nContent-Disposition: form-data; name="a"; filename="x"\r\n\r\n'+b'v'*(21<<20)+b'\r\n'+mpb(1024))
def gen(b,chunk=1<<16):
    for i in range(0,len(b),chunk): yield b[i:i+chunk]
for lab,sz in [('L',L),('L+1',L+1),('L+70K',L+70000),('L+1M',L+(1<<20))]:
    both('chunked raw '+lab,'POST',U(),headers=A,content=gen(raw(sz)))
J={"ci_job_id":"1","name":"j","status":"passed"}
jb=json.dumps({"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"lim","branch":"m","ci_run_id":"J","jobs":[J]}).encode()
JH={**A,'content-type':'application/json'}
def jsz(sz): return jb+b' '*(sz-len(jb))
for lab,sz in [('L',L),('L+1',L+1)]:
    both('json '+lab,'POST','/api/ingest/pipeline',headers=JH,content=jsz(sz))
    both('json chunked '+lab,'POST','/api/ingest/pipeline',headers=JH,content=gen(jsz(sz)))
print('### lying content-length (raw socket)')
def sock(port,head,body,halfclose=True,wait=8):
    s=socket.create_connection(('127.0.0.1',port)); s.settimeout(wait); t=time.time()
    s.sendall(head+body)
    if halfclose: s.shutdown(socket.SHUT_WR)
    out=b''
    try:
        while True:
            d=s.recv(65536)
            if not d: break
            out+=d
    except Exception as e: out+=b'<<'+type(e).__name__.encode()+b'>>'
    s.close()
    first=out.split(b'\r\n')[0][:40]; bd=out.split(b'\r\n\r\n',1)[-1][:90]
    return first.decode('latin1'),out.count(b'HTTP/1.1 '),bd.decode('latin1'),round(time.time()-t,1)
tok=b'X-API-Key: tok-spec-review\r\nHost: x\r\n'
cases=[('CL 100, 66 sent, half-close',b'Content-Length: 100\r\n',XML,True),('CL 100, 66 sent, stay open',b'Content-Length: 100\r\n',XML,False),('CL 21MiB, 66 sent, half-close',b'Content-Length: %d\r\n'%(21<<20),XML,True),('CL 21MiB, 66 sent, stay open',b'Content-Length: %d\r\n'%(21<<20),XML,False),('CL L+1, stay open',b'Content-Length: %d\r\n'%(L+1),XML,False),('CL 10, 66 sent',b'Content-Length: 10\r\n',XML,True),('CL abc',b'Content-Length: abc\r\n',XML,True),('CL -1',b'Content-Length: -1\r\n',XML,True),('CL two values',b'Content-Length: 66\r\nContent-Length: 67\r\n',XML,True),('CL+TE',b'Content-Length: 5\r\nTransfer-Encoding: chunked\r\n',b'%x\r\n'%len(XML)+XML+b'\r\n0\r\n\r\n',True),('chunked bad',b'Transfer-Encoding: chunked\r\n',b'zz\r\n'+XML,True),('chunked truncated',b'Transfer-Encoding: chunked\r\n',b'%x\r\n'%len(XML)+XML,True)]
for i,(lab,h,b,hc) in enumerate(cases):
    head=b'POST /api/ingest?repo=l/s&commit_sha=c&ci_run_id=S%d HTTP/1.1\r\n'%i+tok+h+b'\r\n'
    p=sock(18181,head,b,hc); r=sock(18182,head,b,hc)
    print('same' if p[:3]==r[:3] else 'DIFF',lab,'\n  PY',p,'\n  RS',r)
