use flakeradar_core::parsing::*;
use serde_json::json;
use std::io::{BufRead,Write};
fn unhex(s:&str)->Vec<u8>{(0..s.len()/2).map(|i|u8::from_str_radix(&s[2*i..2*i+2],16).unwrap()).collect()}
fn main(){
  let f=std::fs::File::open(std::env::args().nth(1).unwrap()).unwrap();
  let out=std::io::stdout();let mut out=std::io::BufWriter::new(out.lock());
  for l in std::io::BufReader::new(f).lines(){let l=l.unwrap();let (id,h)=l.split_once('\t').unwrap();
    let b=unhex(h);
    let r=match parse_junit_xml(&b,1){
      Ok(c)=>json!({"cases":c.iter().map(|c|json!({"suite":c.suite,"classname":c.classname,"name":c.name,"status":c.status.as_str(),"duration":c.duration,"message":c.message,"details":c.details,"file":c.file,"line":c.line})).collect::<Vec<_>>()}),
      Err(ParseError::InvalidTime{value})=>json!({"time_error":value}),
      Err(ParseError::NoTestCases)=>json!({"error":"no cases"}),
      Err(e)=>json!({"error":e.to_string()}),
    };
    writeln!(out,"{}",json!({"id":id,"r":r})).unwrap();
  }
}
