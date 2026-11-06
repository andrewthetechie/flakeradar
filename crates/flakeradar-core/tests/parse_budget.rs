//! The parser's heap stays under the documented budget (H3).
//!
//! An upload is untrusted, and one long attribute repeated across many cases
//! used to ask for terabytes (a 10 MiB `<testsuite name>` with 500,000 cases
//! copied the name once per case). The parser now refuses past a budget, so what
//! is proved here is a measured allocation, not a wall-clock time: a counting
//! allocator records the peak live bytes of each parse and the test asserts the
//! bound `flakeradar_core::PARSE_BYTES_MAX` promises.
//!
//! This file holds one test on purpose. `cargo nextest` runs every test in its
//! own process, but `cargo test` runs tests on threads of one process, where a
//! second test's allocations would land in the peak this one reads.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the fixture or the parser is wrong"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use flakeradar_core::{PARSE_BYTES_MAX, ParseError, parse_junit_xml};

/// Bytes currently allocated by any thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// The largest value [`LIVE`] has held since the last reset.
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// An allocator that remembers how much the process holds at once.
struct CountingAllocator;

// SAFETY: every call forwards to the system allocator with the layout it was
// given, and only the byte counters are touched in addition.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc`'s contract for `layout`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` came from `System.alloc` with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The peak heap one parse of `body` holds, with its result.
fn parse_peak(body: &[u8]) -> (Result<usize, ParseError>, usize) {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    let parsed = parse_junit_xml(body, 1).map(|cases| cases.len());
    (parsed, PEAK.load(Ordering::Relaxed))
}

/// Asserts one measurement against the documented budget.
fn within_budget(shape: &str, peak: usize) {
    assert!(
        peak <= PARSE_BYTES_MAX,
        "{shape} held {peak} bytes, over the {PARSE_BYTES_MAX} budget"
    );
}

/// A body at the 20 MiB ingest limit holding `cases` minimal cases.
fn many_small_cases(cases: usize) -> Vec<u8> {
    let mut body = String::from("<testsuite name=\"s\">");
    for _ in 0..cases {
        body.push_str("<testcase name=\"a\"/>");
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

#[test]
fn every_shape_a_body_at_the_ingest_limit_can_take_stays_within_the_budget() {
    // The most Executions an upload admits: 980,000 cases, just under the
    // 1,000,000 node limit, in a 19.6 MB body.
    let body = many_small_cases(980_000);
    assert!(body.len() < 20 * 1024 * 1024, "the body fits the limit");
    let (parsed, peak) = parse_peak(&body);
    assert_eq!(parsed, Ok(980_000));
    within_budget("980,000 minimal cases", peak);

    // A failure-heavy body with retries and details: the shape a real report has.
    let details = "x".repeat(500);
    let case = format!(
        "<testcase classname=\"com.example.LongTestClass\" name=\"test_something\" time=\"0.1\">\
         <flakyFailure message=\"boom\"><stackTrace>{details}</stackTrace></flakyFailure>\
         <failure message=\"boom\">{details}</failure></testcase>"
    );
    let mut body = String::from("<testsuite name=\"s\">");
    while body.len() + 2_000 < 20 * 1024 * 1024 {
        body.push_str(&case);
    }
    body.push_str("</testsuite>");
    let (parsed, peak) = parse_peak(body.as_bytes());
    assert!(parsed.expect("the report parses") > 30_000);
    within_budget("a failure-heavy body", peak);

    // Ten MiB of suite name against sixty cases: without a budget these copies
    // alone pass the whole budget, so the report is refused once they do.
    let mut body = String::from("<testsuite name=\"");
    body.push_str(&"n".repeat(10 * 1024 * 1024));
    body.push_str("\">");
    for _ in 0..60 {
        body.push_str("<testcase name=\"a\"/>");
    }
    body.push_str("</testsuite>");
    let (parsed, peak) = parse_peak(body.as_bytes());
    assert!(
        matches!(parsed, Err(ParseError::InvalidXml { .. })),
        "the amplified report is refused, not parsed: {parsed:?}"
    );
    within_budget("one long identity repeated across cases", peak);
}
