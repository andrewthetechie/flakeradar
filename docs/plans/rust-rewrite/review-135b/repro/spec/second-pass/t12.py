from lib import *
H={**A,'content-type':'application/json'}
for v in ['1e19','1e20','"7"','7.0','true','2147483648','9223372036854775807','0','-1','7.5']:
    both('PUT issue number='+v,'PUT','/api/tests/1/issue',content=('{"number":%s}'%v).encode(),headers=H)
