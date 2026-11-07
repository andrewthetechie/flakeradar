import sys
sys.path.insert(0,'/tmp/python-base/backend')
from app.parsing import parse_junit_xml
def doc(d): return (b'<testsuite>'*d+b'<testcase name="a"/>'+b'</testsuite>'*d)
def test(d):
    try: parse_junit_xml(doc(d),1); return 'ok'
    except RecursionError as e: return 'RecursionError'
    except Exception as e: return type(e).__name__+str(e)[:50]
last=None
for d in range(950,1010):
    r=test(d)
    if r!=last: print('bare script depth',d,r); last=r
# inside deep caller stack, simulating a server
def nest(n,f): return f() if n==0 else nest(n-1,f)
for frames in (20,50,100,200):
    last=None
    for d in range(600,1010):
        r=nest(frames,lambda:test(d))
        if r!=last: print('frames',frames,'depth',d,r); last=r
