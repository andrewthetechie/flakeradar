use flakeradar_core::parse_junit_xml;

fn nested(depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    for _ in 0..depth { xml.push_str("<testsuite>"); }
    xml.push_str(r#"<testcase name="t"/>"#);
    for _ in 0..depth { xml.push_str("</testsuite>"); }
    xml.into_bytes()
}

#[test]
fn deep_nesting() {
    for depth in [500, 1000, 1100, 5000, 100_000, 1_000_000] {
        let body = nested(depth);
        match parse_junit_xml(&body, 1) {
            Ok(cases) => println!("depth {depth}: {} bytes -> ok, {} cases", body.len(), cases.len()),
            Err(error) => println!("depth {depth}: {} bytes -> {}", body.len(), error),
        }
    }
}
