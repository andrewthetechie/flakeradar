import json,sys,time,threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
port=int(sys.argv[1]); log=open(sys.argv[2],'a',buffering=1)
state={'next':100,'delay':0.0,'states':{},'status':{}, 'hang':False}
class H(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*a): pass
    def _send(self,code,obj,hdr=None):
        b=json.dumps(obj).encode() if not isinstance(obj,bytes) else obj
        self.send_response(code); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(b)))
        for k,v in (hdr or {}).items(): self.send_header(k,v)
        self.end_headers(); self.wfile.write(b)
    def handle_any(self):
        n=int(self.headers.get('Content-Length') or 0); body=self.rfile.read(n) if n else b''
        if self.path=='/__cfg':
            state.update(json.loads(body)); return self._send(200,{})
        try: j=json.loads(body) if body else None
        except Exception: j=body.decode('latin1')
        log.write(json.dumps({'t':time.time(),'m':self.command,'p':self.path,'body':j,'h':{k.lower():v for k,v in self.headers.items() if k.lower() in('accept','x-github-api-version','authorization','user-agent','content-type')}})+'\n')
        if state['delay']: time.sleep(state['delay'])
        for pat in state.get('drop',[]):
            if pat in self.command+' '+self.path:
                self.close_connection=True; self.connection.close(); return
        for pat,(code,obj,hdr) in state['status'].items():
            if pat in self.command+' '+self.path:
                return self._send(code,obj,hdr)
        parts=self.path.split('/')
        if self.command=='POST' and self.path.endswith('/issues'):
            state['next']+=1; return self._send(201,{'number':state['next']})
        if self.command=='POST' and self.path.endswith('/comments'):
            return self._send(201,{'id':1})
        if self.command=='PATCH':
            return self._send(200,{'state':'closed'})
        if self.command=='GET':
            num=parts[-1]
            st=state['states'].get(num,{'state':'open'})
            return self._send(200,st)
        self._send(500,{})
    do_GET=do_POST=do_PATCH=handle_any
ThreadingHTTPServer(('127.0.0.1',port),H).serve_forever()
