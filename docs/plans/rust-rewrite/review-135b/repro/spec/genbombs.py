import sys
d=sys.argv[1]
M=1024*1024
LIM=20*M
def w(n,b):
    assert len(b)<=LIM,(n,len(b)); open(f"{d}/{n}.xml","wb").write(b); print(n,len(b))
# a) one entity expanded in failure text many times (text node growth)
ent=b"A"*(4*M)
head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s"><testcase name="a"><failure>'
tail=b'</failure></testcase></testsuite>'
w("text_entity_quadratic",head+b"&e;"*((LIM-len(head)-len(tail))//3)+tail)
# b) same in an attribute
head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s"><testcase name="a" classname="'
tail=b'"/></testsuite>'
w("attr_entity_quadratic",head+b"&e;"*((LIM-len(head)-len(tail))//3)+tail)
# c) per-case capped details through an entity, each case 16 KiB details + 2000 message
ent=b"D"*20000
head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s">'
case=b'<testcase name="a"><failure>&e;</failure></testcase>'
tail=b'</testsuite>'
w("per_case_details_entity",head+case*((LIM-len(head)-len(tail))//len(case))+tail)
# d) identity through an entity under the budget? name holds entity 60 bytes per case; budget should refuse
# e) namespace copying: 65000 prefixes declared on nested scope, then siblings redeclaring one
n=60000
open_=b"".join(b'<x xmlns:p%d="u">'%i for i in range(n))
close=b"</x>"*n
sib=b'<y xmlns:p0="u"/>'
head=b'<testsuite name="s"><testcase name="a"/>'+open_
tail=close+b'</testsuite>'
w("namespace_quadratic",head+sib*((LIM-len(head)-len(tail))//len(sib))+tail)
# f) many attributes on nodes
attrs=b"".join(b' a%d=""'%i for i in range(200))
el=b'<testcase name="a"'+attrs+b'/>'
head=b'<testsuite name="s">';tail=b'</testsuite>'
w("many_attrs",head+el*((LIM-len(head)-len(tail))//len(el))+tail)
# g) unicode digits in a time attribute (CPU)
w("time_math_digits",b'<testsuite><testcase name="a" time="'+("\U0001D7FF"*(5*M-20)).encode()+b'"/></testsuite>')
