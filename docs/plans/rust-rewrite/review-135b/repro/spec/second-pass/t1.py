from lib import *
print("#### query int page_size")
for v in ["1_000","1_0"," 7 ","+3","٣","1e3","7.0","7.00","7.","7.5","0x10","","-0","007","1__0","_1","1_","٣.0","٧٧","\t7\n","+","--1","1.0e0","7.0_0","99999999999999999999999999","-99999999999999999999999999","4294967296","Infinity","NaN","true","٣_٣", " 7"," 7","　7","７"]:
    both(repr(v),'GET','/api/tests',params={'page_size':v})
print("#### query int page huge")
for v in ["9223372036854775807","9223372036854775808","99999999999999999999","1"+"0"*4299,"1"+"0"*4300,"1"+"0"*5000]:
    both('page='+v[:24]+f'(len {len(v)})','GET','/api/tests',params={'page':v})
print("#### repeated/empty/blank optional")
both('new_within_days=','GET','/api/tests',params={'new_within_days':''})
both('page=&page_size=','GET','/api/tests?page=&page_size=')
both('page twice','GET','/api/tests?page=x&page=2')
both('page twice2','GET','/api/tests?page=2&page=x')
both('multi err','GET','/api/tests?sort=z&page=x&page_size=y&include_stable=q&new_within_days=0')
print("#### body ci_run_attempt raw")
job='{"name":"j","status":"success"}'
tmpl='{"repo":"o/r","provider":"github","pipeline":"p","commit_sha":"abc","branch":"m","ci_run_id":"RID","ci_run_attempt":%s,"jobs":[%s]}'
vals=['1e3','1.0','1.5','true','false','"1_0"','" 7 "','"+3"','"٣"','"1e3"','"7.0"','"7.5"','""','9223372036854775807','9223372036854775808','18446744073709551616','1e19','9.3e18','1e20','1e22','1e27','1e28','1e30','1e308','1e400','-1e20','null','[]','{}','2147483647','2147483648','3e9','9007199254740993','9.0e15','9223372036854775000.0','4.611686018427388e18','1E2','"1E2"','"99999999999999999999"']
for i,v in enumerate(vals):
    both('attempt='+v,'POST','/api/ingest/pipeline',content=(tmpl%(v,job)).replace('RID',f'r{i}').encode(),headers={**A,'content-type':'application/json'})
