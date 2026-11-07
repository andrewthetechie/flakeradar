import sys,json
sys.path.insert(0,'/tmp/python-base/backend')
from app.parsing import parse_junit_xml,ParseError
sys.setrecursionlimit(1000)
for l in open(sys.argv[1]):
    i,h=l.rstrip('\n').split('\t')
    b=bytes.fromhex(h)
    try:
        c=parse_junit_xml(b,1)
        r={"cases":[{"suite":x.suite,"classname":x.classname,"name":x.name,"status":x.status,"duration":x.duration if x.duration==x.duration and abs(x.duration)!=float('inf') else repr(x.duration),"message":x.message,"details":x.details,"file":x.file,"line":x.line} for x in c]}
    except ParseError as e:
        r={"error":str(e)}
    except Exception as e:
        r={"crash":type(e).__name__+": "+str(e)[:100]}
    print(json.dumps({"id":i,"r":r}))
