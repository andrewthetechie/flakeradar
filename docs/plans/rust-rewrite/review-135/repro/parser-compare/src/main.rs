use flakeradar_core::*;
use serde_json::json;
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for p in names {
        let bytes = std::fs::read(&p).unwrap();
        let out = match std::panic::catch_unwind(|| parse_junit_xml(&bytes, 1)) {
            Ok(Ok(cases)) => json!({"cases": cases.iter().map(|c| json!({
                "suite":c.suite,"classname":c.classname,"name":c.name,"status":c.status.as_str(),"duration":c.duration,
                "message":c.message,"details":c.details,"file":c.file,"line":c.line,
                "fp":fingerprint(&c.suite,&c.classname,&c.name),
                "cat": if c.status.is_failing(){Some(classify_failure(&c.message,&c.details).as_str())}else{None},
            })).collect::<Vec<_>>()}),
            Ok(Err(e)) => json!({"error": e.to_string()}),
            Err(_) => json!({"panic": true}),
        };
        println!("{}", json!({"file": p.file_name().unwrap().to_str().unwrap(), "r": out}));
    }
}
