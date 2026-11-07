//! The parser's heap stays under the documented budget (H3, SP1).
//!
//! An upload is untrusted, and one long attribute repeated across many cases
//! used to ask for terabytes (a 10 MiB `<testsuite name>` with 500,000 cases
//! copied the name once per case). The parser now refuses past a budget, so what
//! is proved here is a measured allocation, not a wall-clock time: a counting
//! allocator records the peak live bytes of each parse and the test asserts the
//! bound `flakeradar_core::parsing::PARSE_BYTES_MAX` promises, over every shape
//! the second review measured, including the entity expansions and the namespace
//! nesting that used to pass the parser's own limits.
//!
//! This file holds one test on purpose. `cargo nextest` runs every test in its
//! own process, but `cargo test` runs tests on threads of one process, where a
//! second test's allocations would land in the peak this one reads. The
//! allocator is the backstop the review's own harness had: a shape that would
//! exhaust the machine stops the process at [`HARD_LIMIT`] with exit code 42
//! instead of taking the host with it.

// Rust guideline compliant 2026-07-21

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use flakeradar_core::parsing::{PARSE_BYTES_MAX, ParseError, parse_junit_xml};

/// Bytes currently allocated by any thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// The largest value [`LIVE`] has held since the last reset.
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Whether the allocator has already stopped the process.
///
/// Set before the message is written, because writing it allocates: without the
/// flag the allocator would re-enter itself and never reach `exit`.
static EXCEEDED: AtomicBool = AtomicBool::new(false);

/// The most heap one shape may hold before the process stops.
///
/// Twice the budget a parse is allowed: a peak between the budget and this is a
/// test failure with a number in it, and a peak past it is a runaway growth that
/// would not stop on its own. A GiB is cheap to hand out once and impossible to
/// mistake for a real report.
const HARD_LIMIT: usize = 2 * PARSE_BYTES_MAX;

/// The ingest limit a body may fill, in bytes.
const INGEST_LIMIT: usize = 20 * 1024 * 1024;

/// The most heap a body the parser refuses may cost, as a multiple of its own
/// size.
///
/// A refusal happens before the parse, so what it holds is the decoded text (a
/// second copy when a stale encoding declaration makes the reader fall back to
/// lossy UTF-8) and the error message: nothing proportional to the shape it
/// refuses.
const REFUSAL_BODIES_ALLOWED: usize = 3;

/// The heap a refusal costs whatever the body's size.
///
/// A small body's peak is mostly the parser's own start-up: a vector per
/// declaration, node and namespace, and the error message. A quarter of a MiB is
/// several times what a refused bomb of a few hundred bytes needs, and nothing
/// next to the budget for one at the ingest limit.
const REFUSAL_OVERHEAD: usize = 256 * 1024;

/// An allocator that remembers how much the process holds at once.
struct CountingAllocator;

// SAFETY: every call forwards to the system allocator with the layout it was
// given, and only the byte counters are touched in addition.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        if live > HARD_LIMIT && !EXCEEDED.swap(true, Ordering::Relaxed) {
            use std::io::Write as _;
            let _ = std::io::stderr()
                .write_all(b"the parse passed the hard heap limit: it would exhaust the machine\n");
            std::process::exit(42);
        }
        // SAFETY: the caller upholds `GlobalAlloc`'s contract for `layout`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Saturating: `parse_peak` resets the counter to zero while the bodies
        // the test built before it are still alive, so their bytes are freed
        // against a counter that never counted them.
        let _ = LIVE.try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
            Some(live.saturating_sub(layout.size()))
        });
        // SAFETY: `ptr` came from `System.alloc` with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The peak heap one parse of `body` holds, with its answer as a case count.
fn parse_peak(body: &[u8]) -> (Result<usize, ParseError>, usize) {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    let parsed = parse_junit_xml(body, 1).map(|cases| cases.len());
    (parsed, PEAK.load(Ordering::Relaxed))
}

/// What one shape's body must answer.
enum Verdict {
    /// Refused as a bad report before the parse starts, which the ingest route
    /// answers 422.
    Invalid,
    /// Refused as a `time` this server cannot read, which the route answers 400.
    InvalidTime,
    /// Refused as a bad report, but only after the parser copied a budget's worth
    /// of Test identities: this one holds [`PARSE_BYTES_MAX`] rather than a share
    /// of its own size, because the copies are what it is refused for.
    Amplified,
    /// Parsed, holding at least the given number of Executions.
    Parsed(usize),
}

/// One body, and what it must answer.
struct Shape {
    /// The name the second review's table gives the shape.
    name: &'static str,
    /// The body to parse.
    body: Vec<u8>,
    /// What the parse must answer.
    verdict: Verdict,
}

/// An entity of `value` bytes, referenced as often as `limit` allows.
///
/// `text` puts the references in a `<failure>` body; otherwise they fill a
/// `classname` attribute. Both are places `roxmltree` expands a reference in,
/// and neither costs it a node, so its own node limit never sees them.
fn entity_expansion(value: usize, limit: usize, text: bool) -> Vec<u8> {
    let entity = "A".repeat(value);
    let (head, tail) = if text {
        (
            format!(
                "<!DOCTYPE t [<!ENTITY e \"{entity}\">]><testsuite name=\"s\"><testcase name=\"a\"><failure>"
            ),
            "</failure></testcase></testsuite>",
        )
    } else {
        (
            format!(
                "<!DOCTYPE t [<!ENTITY e \"{entity}\">]><testsuite name=\"s\"><testcase name=\"a\" classname=\""
            ),
            "\"/></testsuite>",
        )
    };
    let references = limit.saturating_sub(head.len() + tail.len()) / 3;
    let mut body = head;
    for _ in 0..references {
        body.push_str("&e;");
    }
    body.push_str(tail);
    body.into_bytes()
}

/// Sixty thousand nested namespace declarations, then siblings at that depth
/// that redeclare one of them.
///
/// Every element that declares a namespace makes the parser copy the
/// declarations in scope into its own table, so the nesting costs a quadratic
/// number of entries while each element adds a handful of bytes.
fn nested_namespace_declarations(limit: usize) -> Vec<u8> {
    const NESTING: usize = 60_000;
    let mut body = String::from("<testsuite name=\"s\"><testcase name=\"a\"/>");
    for index in 0..NESTING {
        let _ = write!(body, "<x xmlns:p{index}=\"u\">");
    }
    body.push_str(&"</x>".repeat(NESTING));
    while body.len() + 32 < limit {
        body.push_str("<y xmlns:p0=\"u\"/>");
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

/// One `time` of five million mathematical digit nines: a 20 MiB body of code
/// points that are all decimal digits, so every one of them is rewritten.
fn mathematical_digit_time() -> Vec<u8> {
    let mut body = String::from("<testsuite name=\"s\"><testcase name=\"a\" time=\"");
    body.push_str(&"\u{1d7ff}".repeat(5 * 1024 * 1024 - 64));
    body.push_str("\"/></testsuite>");
    body.into_bytes()
}

/// Two hundred attributes on every case, as many cases as the limit allows.
fn many_attributes(limit: usize) -> Vec<u8> {
    let mut attributes = String::new();
    for index in 0..200 {
        let _ = write!(attributes, " a{index}=\"\"");
    }
    let case = format!("<testcase name=\"a\"{attributes}/>");
    let mut body = String::from("<testsuite name=\"s\">");
    while body.len() + case.len() + 16 < limit {
        body.push_str(&case);
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

/// Nine levels of ten-fold entity expansion: the classic nested bomb.
fn nested_entity_bomb() -> Vec<u8> {
    let mut declarations = String::from("<!ENTITY a \"aaaaaaaaaa\">");
    let mut previous = 'a';
    for name in 'b'..='j' {
        let _ = write!(
            declarations,
            "<!ENTITY {name} \"{}\">",
            format!("&{previous};").repeat(10)
        );
        previous = name;
    }
    format!(
        "<!DOCTYPE testsuite [{declarations}]><testsuite name=\"s\"><testcase classname=\"c\" \
         name=\"t\"><failure message=\"m\">&j;</failure></testcase></testsuite>"
    )
    .into_bytes()
}

/// A body at the ingest limit holding `cases` minimal cases.
fn many_small_cases(cases: usize) -> Vec<u8> {
    let mut body = String::from("<testsuite name=\"s\">");
    for _ in 0..cases {
        body.push_str("<testcase name=\"a\"/>");
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

/// Ten MiB of suite name against sixty cases.
///
/// Without a bound these copies alone pass the whole budget, so the report is
/// refused once they do.
fn one_long_identity() -> Vec<u8> {
    let mut body = String::from("<testsuite name=\"");
    body.push_str(&"n".repeat(10 * 1024 * 1024));
    body.push_str("\">");
    for _ in 0..60 {
        body.push_str("<testcase name=\"a\"/>");
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

/// A failure-heavy report at the ingest limit: the shape a real CI run has.
fn failure_heavy_report(limit: usize) -> Vec<u8> {
    let details = "x".repeat(500);
    let case = format!(
        "<testcase classname=\"com.example.LongTestClass\" name=\"test_something\" time=\"0.1\">\
         <flakyFailure message=\"boom\"><stackTrace>{details}</stackTrace></flakyFailure>\
         <failure message=\"boom\">{details}</failure></testcase>"
    );
    let mut body = String::from("<testsuite name=\"s\">");
    while body.len() + 2_000 < limit {
        body.push_str(&case);
    }
    body.push_str("</testsuite>");
    body.into_bytes()
}

/// Every shape the second review measured, at the size it measured.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            name: "attr_1mib",
            body: entity_expansion(512 * 1024, 1024 * 1024, false),
            verdict: Verdict::Invalid,
        },
        Shape {
            name: "attr_entity_quadratic",
            body: entity_expansion(4 * 1024 * 1024, INGEST_LIMIT, false),
            verdict: Verdict::Invalid,
        },
        Shape {
            name: "text_entity_quadratic",
            body: entity_expansion(4 * 1024 * 1024, INGEST_LIMIT, true),
            verdict: Verdict::Invalid,
        },
        Shape {
            name: "namespace_quadratic",
            body: nested_namespace_declarations(INGEST_LIMIT),
            verdict: Verdict::Invalid,
        },
        Shape {
            name: "time_math_digits",
            body: mathematical_digit_time(),
            verdict: Verdict::InvalidTime,
        },
        Shape {
            name: "many_attrs",
            body: many_attributes(INGEST_LIMIT),
            verdict: Verdict::Parsed(10_000),
        },
        Shape {
            name: "nested_entity_bomb",
            body: nested_entity_bomb(),
            verdict: Verdict::Invalid,
        },
        Shape {
            // The most Executions an upload admits: 980,000 cases, just under
            // the 1,000,000 node limit, in a 19.6 MB body.
            name: "minimal_cases",
            body: many_small_cases(980_000),
            verdict: Verdict::Parsed(980_000),
        },
        Shape {
            name: "long_identity",
            body: one_long_identity(),
            verdict: Verdict::Amplified,
        },
        Shape {
            name: "failure_heavy_report",
            body: failure_heavy_report(INGEST_LIMIT),
            verdict: Verdict::Parsed(30_000),
        },
    ]
}

#[test]
fn every_shape_a_body_can_take_answers_within_the_budget() {
    let shapes = shapes();
    for shape in &shapes {
        let name = shape.name;
        assert!(
            shape.body.len() <= INGEST_LIMIT,
            "{name} is {} bytes, past the ingest limit",
            shape.body.len()
        );
        let (parsed, peak) = parse_peak(&shape.body);
        match shape.verdict {
            Verdict::Invalid => assert!(
                matches!(&parsed, Err(ParseError::InvalidXml { .. })),
                "{name} must be refused as a bad report: {parsed:?}"
            ),
            Verdict::InvalidTime => assert!(
                matches!(&parsed, Err(ParseError::InvalidTime { .. })),
                "{name} must be refused as an unreadable time: {parsed:?}"
            ),
            Verdict::Amplified => assert!(
                matches!(&parsed, Err(ParseError::InvalidXml { .. })),
                "{name} must be refused as a bad report: {parsed:?}"
            ),
            Verdict::Parsed(at_least) => assert!(
                parsed.as_ref().is_ok_and(|cases| *cases >= at_least),
                "{name} must parse at least {at_least} cases: {parsed:?}"
            ),
        }
        let allowed = match shape.verdict {
            Verdict::Parsed(_) | Verdict::Amplified => PARSE_BYTES_MAX,
            _ => shape
                .body
                .len()
                .saturating_mul(REFUSAL_BODIES_ALLOWED)
                .saturating_add(REFUSAL_OVERHEAD)
                .min(PARSE_BYTES_MAX),
        };
        assert!(
            peak <= allowed,
            "{name} held {peak} bytes, over the {allowed} it may hold"
        );
    }
}
