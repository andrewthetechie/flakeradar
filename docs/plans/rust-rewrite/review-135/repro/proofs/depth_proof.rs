//! Proof for H0/S3, rewritten after the fix: nesting past the reference's own
//! recursion limit is refused instead of reaching the process's stack.
//!
//! At `f9330fd` depth 995 and beyond overflowed a 2 MiB stack. `SUITE_DEPTH_MAX`
//! is 990, the deepest nesting the reference could still parse.

use flakeradar_core::parsing::{ParseError, parse_junit_xml};

fn nested(depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    for _ in 0..depth {
        xml.push_str("<testsuite>");
    }
    xml.push_str(r#"<testcase name="t"/>"#);
    for _ in 0..depth {
        xml.push_str("</testsuite>");
    }
    xml.into_bytes()
}

#[test]
fn deep_nesting_is_refused() {
    for depth in [500, 990, 1000, 5000, 100_000] {
        let body = nested(depth);
        // The 2 MiB stack `parse_upload` runs on, so a recursion would die here.
        let answer = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || match parse_junit_xml(&body, 1) {
                Ok(cases) => format!("ok, {} cases", cases.len()),
                Err(error) => format!("Err({error})"),
            })
            .expect("the thread starts")
            .join()
            .expect("no depth reaches the stack");
        println!("depth {depth} -> {answer}");
        if depth <= 990 {
            assert!(answer.starts_with("ok"), "depth {depth} must parse");
        } else {
            assert!(
                answer.starts_with("Err(Not a valid JUnit XML report"),
                "depth {depth} must be refused"
            );
        }
    }
}
