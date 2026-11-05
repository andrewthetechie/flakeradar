use flakeradar_core::parse_junit_xml;

fn body(kind: &str, depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    match kind {
        // Nested suites: the path `collect_cases_named` recurses on.
        "suites" => {
            for _ in 0..depth { xml.push_str("<testsuite>"); }
            xml.push_str(r#"<testcase name="t"/>"#);
            for _ in 0..depth { xml.push_str("</testsuite>"); }
        }
        // Equally deep, in elements that function does not recurse into.
        _ => {
            xml.push_str(r#"<testsuite><testcase name="t">"#);
            for _ in 0..depth { xml.push_str("<x>"); }
            for _ in 0..depth { xml.push_str("</x>"); }
            xml.push_str("</testcase></testsuite>");
        }
    }
    xml.into_bytes()
}

fn main() {
    let kind = std::env::args().nth(1).unwrap();
    let depth: usize = std::env::args().nth(2).unwrap().parse().unwrap();
    let data = body(&kind, depth);
    // tokio's blocking pool gives each thread 2 MiB by default, the stack `parse_upload` runs on.
    let handle = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(move || {
        match parse_junit_xml(&data, 1) {
            Ok(cases) => println!("{kind} depth {depth}: {} bytes -> ok, {} cases", data.len(), cases.len()),
            Err(error) => println!("{kind} depth {depth}: {} bytes -> Err({error})", data.len()),
        }
    }).unwrap();
    handle.join().unwrap();
}
