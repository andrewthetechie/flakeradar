# Writes corpus.tsv: id \t hex
import sys
out=open(sys.argv[1],"w")
def add(i,b):
    if isinstance(b,str): b=b.encode()
    out.write(f"{i}\t{b.hex()}\n")
T='<testsuite name="s"><testcase name="a"/></testsuite>'
def doc(name="a",extra="",body=""):
    return f'<testsuite name="s"><testcase name="{name}" {extra}>{body}</testcase></testsuite>'
# --- declarations
add("xmlstyle_pi_with_encoding",'<?xml-stylesheet href="a" encoding="bogus"?>'+T)
add("xmlfoo_pi",'<?xml-foo encoding="utf-16"?>'+T)
add("pi_XML_upper",'<?XML version="1.0"?>'+T)
add("decl_leading_ws",'\n<?xml version="1.0"?>'+T)
add("decl_v11",'<?xml version="1.1"?>'+T)
add("decl_v2",'<?xml version="2.0"?>'+T)
add("decl_noversion",'<?xml encoding="utf-8"?>'+T)
add("decl_standalone_bad",'<?xml version="1.0" standalone="maybe"?>'+T)
add("decl_order",'<?xml encoding="utf-8" version="1.0"?>'+T)
add("decl_enc_empty",'<?xml version="1.0" encoding=""?>'+T)
add("decl_enc_space",'<?xml version="1.0" encoding=" utf-8"?>'+T)
add("decl_enc_trail",'<?xml version="1.0" encoding="utf-8 "?>'+T)
add("decl_mid_doc",T+'<?xml version="1.0"?>')
add("pi_in_doc",'<testsuite><?xml version="1.0"?><testcase name="a"/></testsuite>')
add("bom_utf8",b'\xef\xbb\xbf'+T.encode())
add("bom_utf8_invalid_byte",b'\xef\xbb\xbf<testsuite><testcase name="a\xff"/></testsuite>')
add("bom_utf8_decl_latin1",b'\xef\xbb\xbf<?xml version="1.0" encoding="iso-8859-1"?><testsuite><testcase name="\xc3\xa9"/></testsuite>')
add("bom_utf8_decl_cp1252",b'\xef\xbb\xbf<?xml version="1.0" encoding="windows-1252"?><testsuite><testcase name="\xc3\xa9"/></testsuite>')
add("bom_utf8_decl_utf16",b'\xef\xbb\xbf<?xml version="1.0" encoding="utf-16"?><testsuite><testcase name="\xc3\xa9"/></testsuite>')
add("bom_utf8_decl_ascii",b'\xef\xbb\xbf<?xml version="1.0" encoding="us-ascii"?><testsuite><testcase name="a"/></testsuite>')
for enc,bom,c in [("utf-16",b'\xff\xfe',"utf-16-le"),("utf-16",b'\xfe\xff',"utf-16-be"),("utf-16le",b'\xff\xfe',"utf-16-le"),("utf-16be",b'\xfe\xff',"utf-16-be"),("utf-8",b'\xff\xfe',"utf-16-le"),("iso-8859-1",b'\xff\xfe',"utf-16-le"),("bogus",b'\xff\xfe',"utf-16-le")]:
    add(f"u16_{enc}_{c}_bom",bom+f'<?xml version="1.0" encoding="{enc}"?><testsuite><testcase name="é"/></testsuite>'.encode(c))
    add(f"u16_{enc}_{c}_nobom",f'<?xml version="1.0" encoding="{enc}"?><testsuite><testcase name="é"/></testsuite>'.encode(c))
add("u16_nodecl_le",'<testsuite><testcase name="é"/></testsuite>'.encode('utf-16-le'))
add("u16_nodecl_be",'<testsuite><testcase name="é"/></testsuite>'.encode('utf-16-be'))
add("u16_odd",b'\xff\xfe'+'<testsuite><testcase name="é"/></testsuite>'.encode('utf-16-le')+b'x')
add("u16_lone_surrogate",b'\xff\xfe'+'<testsuite><testcase name="'.encode('utf-16-le')+b'\x00\xd8'+'"/></testsuite>'.encode('utf-16-le'))
add("u16_ascii_declared",'<?xml version="1.0" encoding="utf-16"?><testsuite><testcase name="é"/></testsuite>'.encode())
add("u16le_ascii_declared",'<?xml version="1.0" encoding="utf-16le"?><testsuite><testcase name="é"/></testsuite>'.encode())
add("utf32",'<testsuite><testcase name="a"/></testsuite>'.encode('utf-32'))
# --- XML well-formedness
wf={
"ctrl_raw_text":doc(body="<failure>a\x01b</failure>"),
"ctrl_raw_attr":doc(name="a\x01b"),
"vt_raw_text":doc(body="<failure>a\x0bb</failure>"),
"fffe_raw":doc(name="a￾b"),
"ffff_raw":doc(name="a￿b"),
"del_raw":doc(name="a\x7fb"),
"c1_raw":doc(name="a\x85b"),
"nbsp":doc(name="a\xa0b"),
"cdata_end_in_text":doc(body="<failure>a]]>b</failure>"),
"dashdash_comment":'<testsuite><!-- a -- b --><testcase name="a"/></testsuite>',
"comment_end_dash":'<testsuite><!-- a ---><testcase name="a"/></testsuite>',
"pi_noname":'<testsuite><? x?><testcase name="a"/></testsuite>',
"pi_xml_name":'<testsuite><?xMl x?><testcase name="a"/></testsuite>',
"lt_in_attr":doc(name="a<b"),
"gt_in_attr":doc(name="a>b"),
"amp_in_attr":doc(name="a&b"),
"apos_quot":'<testsuite><testcase name=\'a"b\'/></testsuite>',
"dup_attr":'<testsuite><testcase name="a" name="b"/></testsuite>',
"dup_attr_ns":'<testsuite xmlns:a="u" xmlns:b="u"><testcase a:x="1" b:x="2" name="n"/></testsuite>',
"no_attr_ws":'<testsuite name="a"name2="b"><testcase name="a"/></testsuite>',
"unbound_prefix_el":'<testsuite><j:testcase name="a"/><testcase name="b"/></testsuite>',
"unbound_prefix_attr":'<testsuite><testcase j:x="1" name="b"/></testsuite>',
"xmlns_empty_prefix":'<testsuite xmlns:a=""><testcase name="b"/></testsuite>',
"xmlns_xml_wrong":'<testsuite xmlns:xml="u"><testcase name="b"/></testsuite>',
"xmlns_xml_right":'<testsuite xmlns:xml="http://www.w3.org/XML/1998/namespace"><testcase name="b"/></testsuite>',
"xmlns_xmlns":'<testsuite xmlns:xmlns="u"><testcase name="b"/></testsuite>',
"xml_ns_bound":'<testsuite xmlns:a="http://www.w3.org/XML/1998/namespace"><testcase name="b"/></testsuite>',
"xmlns_default_empty":'<testsuite xmlns=""><testcase name="b"/></testsuite>',
"xmlns_on_case":'<testsuite><testcase xmlns="urn:x" name="b"/><testcase name="c"/></testsuite>',
"xmlns_on_failure":'<testsuite><testcase name="b"><failure xmlns="urn:x"/></testcase></testsuite>',
"xmlns_undo":'<testsuite xmlns="urn:x"><testcase xmlns="" name="b"/></testsuite>',
"xmlns_prefix_root":'<j:testsuite xmlns:j="u"><testcase name="b"/></j:testsuite>',
"xmlns_suites_prefix":'<j:testsuites xmlns:j="u"><testsuite><testcase name="b"/></testsuite></j:testsuites>',
"colon_name":'<testsuite><a:b:c xmlns:a="u"/><testcase name="b"/></testsuite>',
"colon_start":'<testsuite><:a/><testcase name="b"/></testsuite>',
"colon_attr":'<testsuite><testcase :a="1" name="b"/></testsuite>',
"tag_digit":'<testsuite><1a/><testcase name="b"/></testsuite>',
"tag_case":'<TestSuite><testcase name="b"/></TestSuite>',
"mismatch":'<testsuite><testcase name="b"></testsuite></testcase>',
"trailing_text":T+"x",
"trailing_ws":T+"\n\n",
"trailing_comment":T+"<!-- c -->",
"trailing_pi":T+"<?p q?>",
"two_roots":T+T,
"leading_text":"x"+T,
"leading_ws":"  \n"+T,
"leading_comment":"<!-- c -->"+T,
"empty":"",
"ws_only":"   ",
"cdata_outside":"<![CDATA[x]]>"+T,
"nul_byte":b'<testsuite><testcase name="a\x00"/></testsuite>',
"crlf_text":doc(body="<failure>a\r\nb\rc</failure>"),
"crlf_attr":doc(name="a\r\nb\rc\td\ne"),
"charref_attr_ws":doc(name="a&#10;b&#13;c&#9;d&#32;e"),
"charref_text_cr":doc(body="<failure>a&#13;&#10;b</failure>"),
"cdata_text":doc(body="<failure><![CDATA[a]]><![CDATA[b]]>c</failure>"),
"cdata_crlf":doc(body="<failure><![CDATA[a\r\nb]]></failure>"),
"failure_entity_text":doc(body="<failure>a&amp;b&lt;c&gt;d&apos;e&quot;f</failure>"),
# char refs
**{f"cr_{k}":doc(name=f"a&#{k};b") for k in ["0","1","x1","8","x9","xA","xB","xC","xD","xE","x1F","x20","x7F","x80","x85","x9F","xD7FF","xD800","xDBFF","xDC00","xDFFF","xE000","xFFFD","xFFFE","xFFFF","x10000","x10FFFF","x110000","xFFFFFFFF","x100000000","99999999999999999999","X41","x","","x41 ","0065","00000000000000000000065","-1","+65"]},
**{f"crtext_{k}":doc(body=f"<failure>a&#{k};b</failure>") for k in ["0","1","xFFFE","xFFFF","xD800","x110000","x85"]},
"cr_in_comment":'<testsuite><!-- &#xD800; --><testcase name="a"/></testsuite>',
"cr_in_cdata":doc(body="<failure><![CDATA[&#xD800;]]></failure>"),
"cr_in_pi":'<testsuite><?p &#xD800;?><testcase name="a"/></testsuite>',
"cr_entity_decl":'<!DOCTYPE t [<!ENTITY e "&#xD800;">]>'+T,
"cr_entity_decl_used":'<!DOCTYPE t [<!ENTITY e "&#xD800;">]>'+doc(name="&e;"),
"cr_entity_ctrl":'<!DOCTYPE t [<!ENTITY e "&#1;">]>'+doc(name="&e;"),
"cr_dtd_comment":'<!DOCTYPE t [<!-- &#xD800; -->]>'+T,
"cr_attlist":'<!DOCTYPE t [<!ATTLIST testcase x CDATA "&#xD800;">]>'+T,
"cr_literal_sysid":'<!DOCTYPE t SYSTEM "&#xD800;">'+T,
"cr_after_cdata_like_comment":'<testsuite><testcase name="<!--&#xD800;-->"/></testsuite>',
# doctype & entities
"dt_benign":'<!DOCTYPE testsuite>'+T,
"dt_system":'<!DOCTYPE testsuite SYSTEM "x.dtd">'+T,
"dt_public":'<!DOCTYPE testsuite PUBLIC "-//X//Y" "x.dtd">'+T,
"dt_internal_empty":'<!DOCTYPE testsuite []>'+T,
"dt_elements":'<!DOCTYPE testsuite [<!ELEMENT testsuite ANY><!ATTLIST testcase file CDATA "dflt.py">]>'+T,
"dt_attlist_default_name":'<!DOCTYPE testsuite [<!ATTLIST testcase classname CDATA "dflt">]>'+T,
"dt_attlist_nmtoken":'<!DOCTYPE testsuite [<!ATTLIST testcase name NMTOKEN #IMPLIED>]><testsuite><testcase name="  a  b "/></testsuite>',
"dt_notation":'<!DOCTYPE testsuite [<!NOTATION n SYSTEM "x">]>'+T,
"dt_after_root":T+'<!DOCTYPE testsuite>',
"dt_twice":'<!DOCTYPE testsuite><!DOCTYPE testsuite>'+T,
"dt_name_mismatch":'<!DOCTYPE other>'+T,
"dt_noname":'<!DOCTYPE>'+T,
"dt_lower":'<!doctype testsuite>'+T,
"dt_decl_after":'<!DOCTYPE testsuite><?xml version="1.0"?>'+T,
"dt_comment_pi":'<!DOCTYPE testsuite [<!-- c --><?p q?>]>'+T,
"ent_attr":'<!DOCTYPE t [<!ENTITY e "x">]>'+doc(name="&e;"),
"ent_text":'<!DOCTYPE t [<!ENTITY e "x">]>'+doc(body="<failure>a&e;b</failure>"),
"ent_markup":'<!DOCTYPE t [<!ENTITY e "<testcase name=\'z\'/>">]><testsuite>&e;</testsuite>',
"ent_markup_failure":'<!DOCTYPE t [<!ENTITY e "<failure message=\'m\'>b</failure>">]>'+doc(body="&e;"),
"ent_unbalanced":'<!DOCTYPE t [<!ENTITY e "<a>">]><testsuite>&e;</a><testcase name="b"/></testsuite>',
"ent_lt_attr":'<!DOCTYPE t [<!ENTITY e "a&#60;b">]>'+doc(name="&e;"),
"ent_lt_attr2":'<!DOCTYPE t [<!ENTITY e "<">]>'+doc(name="&e;"),
"ent_redefine":'<!DOCTYPE t [<!ENTITY e "1"><!ENTITY e "2">]>'+doc(name="&e;"),
"ent_predef_redefine":'<!DOCTYPE t [<!ENTITY lt "&#38;#60;">]>'+doc(name="&lt;"),
"ent_predef_redefine_bad":'<!DOCTYPE t [<!ENTITY amp "x">]>'+doc(name="&amp;"),
"ent_undefined":doc(name="&nope;"),
"ent_undefined_dt":'<!DOCTYPE t>'+doc(name="&nope;"),
"ent_undefined_text_dtsys":'<!DOCTYPE t SYSTEM "x.dtd">'+doc(body="<failure>&nope;</failure>"),
"ent_undefined_attr_dtsys":'<!DOCTYPE t SYSTEM "x.dtd">'+doc(name="&nope;"),
"ent_undefined_standalone":'<?xml version="1.0" standalone="yes"?><!DOCTYPE t SYSTEM "x.dtd">'+doc(name="&nope;"),
"ent_external_text":'<!DOCTYPE t [<!ENTITY x SYSTEM "file:///etc/hostname">]>'+doc(body="<failure>a&x;b</failure>"),
"ent_external_attr":'<!DOCTYPE t [<!ENTITY x SYSTEM "file:///etc/hostname">]>'+doc(name="&x;"),
"ent_external_unused":'<!DOCTYPE t [<!ENTITY x SYSTEM "file:///etc/hostname">]>'+T,
"ent_public":'<!DOCTYPE t [<!ENTITY x PUBLIC "-//X" "y">]>'+doc(body="<failure>&x;</failure>"),
"ent_unparsed":'<!DOCTYPE t [<!NOTATION n SYSTEM "x"><!ENTITY u SYSTEM "y" NDATA n>]>'+doc(name="&u;"),
"ent_param_used":'<!DOCTYPE t [<!ENTITY % p "<!ENTITY e \'x\'>"> %p;]>'+doc(name="&e;"),
"ent_param_blocks":'<!DOCTYPE t [<!ENTITY % p "x"> <!ENTITY e "y">]>'+doc(name="&e;"),
"ent_after_pe":'<!DOCTYPE t [<!ENTITY % p SYSTEM "x"> %p; <!ENTITY e "y">]>'+doc(name="&e;"),
"ent_self":'<!DOCTYPE t [<!ENTITY e "&e;">]>'+doc(name="&e;"),
"ent_mutual":'<!DOCTYPE t [<!ENTITY a "&b;"><!ENTITY b "&a;">]>'+doc(name="&a;"),
"ent_self_unused":'<!DOCTYPE t [<!ENTITY e "&e;">]>'+T,
"ent_11_deep":'<!DOCTYPE t ['+''.join(f'<!ENTITY e{i} "&e{i+1};">' for i in range(11))+'<!ENTITY e11 "x">]>'+doc(name="&e0;"),
"ent_300_refs":'<!DOCTYPE t [<!ENTITY b "x"><!ENTITY a "'+'&b;'*300+'">]>'+doc(name="&a;"),
"ent_nested_text":'<!DOCTYPE t [<!ENTITY b "y"><!ENTITY a "x&b;z">]>'+doc(body="<failure>&a;</failure>"),
"ent_charref_value":'<!DOCTYPE t [<!ENTITY e "&#65;&#38;&#60;">]>'+doc(body="<failure>&e;</failure>"),
"ent_percent_value":'<!DOCTYPE t [<!ENTITY e "100&#37;">]>'+doc(name="&e;"),
"ent_in_entity_value_later":'<!DOCTYPE t [<!ENTITY a "&b;"><!ENTITY b "x">]>'+doc(name="&a;"),
"ent_wrapped_ws":'<!DOCTYPE t [<!ENTITY e "a\nb\tc">]>'+doc(name="&e;"),
"ent_outside_root":'<!DOCTYPE t [<!ENTITY e "x">]>&e;'+T,
"bomb_classic":'<!DOCTYPE t [<!ENTITY a "aaaaaaaaaa">'+''.join(f'<!ENTITY {chr(98+i)} "'+f'&{chr(97+i)};'*10+'">' for i in range(9))+']>'+doc(name="&j;"),
"bomb_small3":'<!DOCTYPE t [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "'+'&a;'*10+'"><!ENTITY c "'+'&b;'*10+'">]>'+doc(name="&c;"),
# structure
"failure_grandchild":doc(body="<system-out><failure message='m'/></system-out>"),
"failure_in_properties":doc(body="<properties><error/></properties>"),
"failure_in_flaky":doc(body="<flakyFailure message='r'><failure message='m'/></flakyFailure>"),
"skipped_grandchild":doc(body="<x><skipped/></x>"),
"failure_grandchild_then_child":doc(body="<x><error message='e'/></x><failure message='f'/>"),
"multi_result":doc(body="<skipped message='s'/><failure message='f'/>"),
"failure_type_only":doc(body="<failure type='T'/>"),
"failure_ws":doc(body="<failure message='  m \n'>  b \n c  </failure>"),
"retry_stack_nested":doc(body="<rerunFailure><stackTrace>a<x/>b</stackTrace></rerunFailure><failure/>"),
"retry_nested_retry":doc(body="<x><flakyFailure/></x>"),
"retry_after_time":doc(body="<failure/><rerunError time='1,5'/><rerunFailure time='NaN'/>"),
"retry_time_inf":doc(body="<flakyFailure time='inf'/>"),
"retry_time_bad":doc(body="<flakyFailure time='abc'/>"),
"retry_time_empty":doc(body="<flakyFailure time=''/>"),
"retry_time_ws":doc(body="<flakyFailure time=' 2 '/>"),
"retry_time_us":doc(body="<flakyFailure time='1_0'/>"),
"system_out_twice":doc(body="<failure/><system-out>a</system-out><system-out>b</system-out>"),
"system_out_child":doc(body="<failure/><system-out>a<x/>b</system-out>"),
"system_out_ns":doc(body="<failure/><system-out xmlns='u'>a</system-out>"),
"suite_in_case":doc(body="<testsuite><testcase name='z'/></testsuite>"),
"suites_case_direct":'<testsuites><testcase name="a"/><testsuite><testcase name="b"/></testsuite></testsuites>',
"suites_nested_suites":'<testsuites><testsuites><testsuite><testcase name="a"/></testsuite></testsuites></testsuites>',
"suite_nested_mixed":'<testsuite name="o"><testsuite name="i"><testcase name="1"/></testsuite><testcase name="2"/><testsuite name="j"><testcase name="3"/></testsuite></testsuite>',
"suites_empty":'<testsuites/>',
"name_empty":doc(name=""),
"no_name":'<testsuite><testcase classname="c"/></testsuite>',
"name_ws":doc(name="  a  "),
"suite_noname":'<testsuite><testcase name="a"/></testsuite>',
"ns_attr_only_name":'<testsuite xmlns:j="u"><testcase j:name="x"/></testsuite>',
"xml_attr":'<testsuite><testcase xml:name="x" name="a"/></testsuite>',
"foo_junit":'<foo/>',
"marker_exact":doc(body="<flakyFailure/><failure message='test passed on attempt 2/3 but is configured to fail when flaky'/>"),
"marker_no_retry":doc(body="<failure message='test passed on attempt 2/3 but is configured to fail when flaky'/>"),
"marker_arabic_digits":doc(body="<flakyFailure/><failure message='test passed on attempt ٢/٣ but is configured to fail when flaky'/>"),
"marker_err":doc(body="<flakyError/><error message='test passed on attempt 2/3 but is configured to fail when flaky'/>"),
"marker_newline":doc(body="<flakyFailure/><failure>test passed on attempt 2/3 but is configured to fail when flaky\n</failure>"),
"marker_rerun":doc(body="<rerunFailure/><failure message='test passed on attempt 2/3 but is configured to fail when flaky'/>"),
}
for k,v in wf.items(): add(k,v)
# time
for t in ["","  ","0","1.5","-1","-0","+1","1e3","1E3",".5","5.","1_000","1__0","_1","1_","1,000.5",",",",,","1,","NaN","nan","-nan","inf","-inf","+inf","Infinity","INF","1e999","-1e999","1e-999","0x10","１２","٣","𝟗","१२","1 2"," 1 ","\t1\n","\xa01\xa0","1 2","abc","1.2.3","1e","e1","1d","1f","1L","٣.٥","٣_٣","1_٣"]:
    add("time_"+t.encode('unicode_escape').decode().replace('\t','T').replace(' ','S'),f'<testsuite><testcase name="a" time="{t}"/></testsuite>')
# line/file
for l in ["1","0","-1","+1"," 1 ","1.0","٣","２","²","","x","2147483647","2147483648","99999999999999999999","0x1","1_0"]:
    add("line_"+l.encode('unicode_escape').decode().replace(' ','S'),f'<testsuite><testcase name="a" file="f.py" line="{l}"/></testsuite>')
for f in ["./a.py","././a.py","  ./a.py ",".//a.py","","  ",".","./"]:
    add("file_"+f.replace('/','S').replace(' ','_'),f'<testsuite><testcase name="a" file="{f}" line="1"/></testsuite>')
# strip semantics in message
for ch in ["\u0085"," "," ","　"," "," ","​","﻿","᠎"," "]:
    add(f"strip_{ord(ch):x}",doc(body=f"<failure message='{ch}m{ch}'>{ch}l1{ch}\nl2</failure>"))
    add(f"firstline_{ord(ch):x}",doc(body=f"<failure>l1{ch}l2</failure>"))
out.close()
