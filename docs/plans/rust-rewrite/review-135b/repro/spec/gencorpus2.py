import sys,encodings.aliases
out=open(sys.argv[1],"w")
def add(i,b):
    if isinstance(b,str): b=b.encode()
    out.write(f"{i}\t{b.hex()}\n")
# xmlns="" forms
add('xe_root','<testsuite xmlns=""><testcase name="a"/></testsuite>')
add('xe_suites','<testsuites xmlns=""><testsuite><testcase name="a"/></testsuite></testsuites>')
add('xe_case','<testsuite><testcase xmlns="" name="a"/></testsuite>')
add('xe_failure','<testsuite><testcase name="a"><failure xmlns="" message="m">b</failure></testcase></testsuite>')
add('xe_sysout','<testsuite><testcase name="a"><failure message="m"/><system-out xmlns="">o</system-out></testcase></testsuite>')
add('xe_undo_under_default_case','<x xmlns="urn:x"/>')
add('xe_suite_nested','<testsuites><testsuite xmlns=""><testcase name="a"/></testsuite></testsuites>')
add('xe_retry','<testsuite><testcase name="a"><flakyFailure xmlns="" message="m"/></testcase></testsuite>')
# sweep labels x bytes
labels=sorted(set(list(encodings.aliases.aliases.keys())+list(encodings.aliases.aliases.values())))
labels+=["windows-1252","windows-1250","windows-1251","windows-1253","windows-1254","windows-1255","windows-1256","windows-1257","windows-1258","windows-874","iso-8859-2","iso-8859-3","iso-8859-4","iso-8859-5","iso-8859-6","iso-8859-7","iso-8859-8","iso-8859-8-i","iso-8859-9","iso-8859-10","iso-8859-11","iso-8859-13","iso-8859-14","iso-8859-15","iso-8859-16","koi8-r","koi8-u","macintosh","x-mac-cyrillic","ibm866","cp866","tis-620","x-cp1252","x-user-defined","x-sjis","shift_jis","euc-jp","euc-kr","gbk","gb2312","gb18030","big5","iso-2022-jp","replacement","unicode-1-1-utf-8","utf8","UTF8","Utf-8","x-unicode20utf8","logical","visual","l5","latin5","latin6","latin9","ansi","us","646","mac-roman","hp-roman8","cp437","cp850","cp037","cp1140","utf-7","utf_7","rot13","hex","base64","idna","punycode","undefined","mbcs","oem","raw_unicode_escape","unicode_escape","unicode-escape","utf-32","utf-32le","utf-8-sig","utf_8_sig"]
labels=sorted(set(labels))
for lab in labels:
    if any(c in lab for c in '"<>&'): continue
    for b in list(range(0x80,0x100,1)):
        add(f"sw|{lab}|{b:02x}",b'<?xml version="1.0" encoding="'+lab.encode()+b'"?><testsuite><testcase name="n'+bytes([b])+b'"/></testsuite>')
    add(f"sw|{lab}|ascii",b'<?xml version="1.0" encoding="'+lab.encode()+b'"?><testsuite><testcase name="n"/></testsuite>')
out.close()
