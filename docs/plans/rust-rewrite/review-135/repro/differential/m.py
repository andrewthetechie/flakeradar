import httpx,difflib
a=httpx.get('http://127.0.0.1:18081/metrics').text.splitlines(); b=httpx.get('http://127.0.0.1:18082/metrics').text.splitlines()
for l in difflib.unified_diff(a,b,lineterm='',n=0): print(l[:200])
