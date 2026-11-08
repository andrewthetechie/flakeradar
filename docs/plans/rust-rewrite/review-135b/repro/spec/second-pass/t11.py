import sys
sys.argv=['x']
exec(open('t8.py').read().split("PW='s3cretpw'")[0])
PW='s3cretpw'
for lab,u in [('slash pw, good port','postgresql://u:%s/x@db:5432/d'%PW),('base64-style','postgresql://u:ab+c/%s==@db:5432/d'%PW),('query pw + bad port','postgresql://u@db:notaport/d?password=%s'%PW),('encoded slash ok','postgresql://u:%s%%2Fx@127.0.0.1:1/d'%PW)]:
    rc,out=rs({'FLAKERADAR_DATABASE_URL':u}); print(lab,'rc',rc,'LEAK' if PW in out else 'clean','|',out.strip().splitlines()[-1][:150] if out.strip() else '')
