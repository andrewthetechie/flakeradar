//! Proof for H0/S3 in the release profile, rewritten after the fix: no depth
//! reaches the stack, and a body that is deep but not nested suites still parses.
//!
//! Run as `cargo run --profile relproof --bin release_depth_main -- suites 200000`
//! in the scratch crate; at `f9330fd` the first two arguments aborted the process.

use flakeradar_core::parsing::parse_junit_xml;

fn body(kind: &str, depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    if kind == "suites" {
        for _ in 0..depth {
            xml.push_str("<testsuite>");
        }
        xml.push_str(r#"<testcase name="t"/>"#);
        for _ in 0..depth {
            xml.push_str("</testsuite>");
        }
    } else {
        // Equally deep, in elements the suite walk does not recurse into.
        xml.push_str(r#"<testsuite><testcase name="t">"#);
        for _ in 0..depth {
            xml.push_str("<x>");
        }
        for _ in 0..depth {
            xml.push_str("</x>");
        }
        xml.push_str("</testcase></testsuite>");
    }
    xml.into_bytes()
}

fn main() {
    let kind = std::env::args().nth(1).expect("a shape: suites or elements");
    let depth: usize = std::env::args()
        .nth(2)
        .expect("a depth")
        .parse()
        .expect("a number");
    let data = body(&kind, depth);
    // The 2 MiB stack a blocking parse runs on.
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || match parse_junit_xml(&data, 1) {
            Ok(cases) => println!(
                "{kind} depth {depth}: {} bytes -> ok, {} cases",
                data.len(),
                cases.len()
            ),
            Err(error) => println!("{kind} depth {depth}: {} bytes -> Err({error})", data.len()),
        })
        .expect("the thread starts");
    handle.join().expect("no depth reaches the stack");
}
