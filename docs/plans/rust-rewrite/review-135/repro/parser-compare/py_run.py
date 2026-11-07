import sys,os,json
sys.path.insert(0, sys.argv[2])
from app.parsing import parse_junit_xml, fingerprint, ParseError
from app.classify import classify_failure
d=sys.argv[1]
for f in sorted(os.listdir(d)):
    b=open(os.path.join(d,f),'rb').read()
    try:
        cases=parse_junit_xml(b,1)
        r={"cases":[{"suite":c.suite,"classname":c.classname,"name":c.name,"status":c.status,"duration":c.duration,"message":c.message,"details":c.details,"file":c.file,"line":c.line,"fp":fingerprint(c.suite,c.classname,c.name),"cat":classify_failure(c.message,c.details) if c.status in("failed","error") else None} for c in cases]}
    except ParseError as e:
        r={"error":str(e)}
    except Exception as e:
        r={"crash":type(e).__name__+": "+str(e)}
    print(json.dumps({"file":f,"r":r}))
