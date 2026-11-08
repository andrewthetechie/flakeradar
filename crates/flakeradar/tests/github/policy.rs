//! The one status table: verdict by verdict, operation by operation.

// Rust guideline compliant 2026-07-21

use flakeradar::github;

/// One answered call, as [`github::Answer::classify`] reads it.
fn classified(status: u16, headers: &[(&str, &str)], success: github::Success) -> github::Verdict {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        map.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
            reqwest::header::HeaderValue::from_str(value).expect("a header value"),
        );
    }
    github::Answer::new(status, map, String::new()).classify(success)
}

#[test]
fn classify_names_every_answer_as_one_verdict() {
    use github::{Success, Verdict};

    // The status a call asks for is its success; any other 2xx is a failure each
    // operation reads for itself, which is why a close's 201 and a create's 200
    // both land on `Other`.
    assert_eq!(classified(201, &[], Success::Created), Verdict::Accepted);
    assert_eq!(classified(200, &[], Success::Fetched), Verdict::Accepted);
    assert_eq!(classified(201, &[], Success::Fetched), Verdict::Other);
    assert_eq!(classified(200, &[], Success::Created), Verdict::Other);
    assert_eq!(classified(204, &[], Success::Created), Verdict::Other);
    assert_eq!(
        classified(401, &[], Success::Created),
        Verdict::Unauthorized
    );
    assert_eq!(
        classified(403, &[], Success::Created),
        Verdict::RepoDenied,
        "a permission 403 is not throttling"
    );
    assert_eq!(
        classified(403, &[("x-ratelimit-remaining", "0")], Success::Created),
        Verdict::RateLimited,
        "the rate limit is decided first, as every batch stops on it"
    );
    assert_eq!(classified(429, &[], Success::Created), Verdict::RateLimited);
    assert_eq!(classified(404, &[], Success::Created), Verdict::NotFound);
    assert_eq!(classified(410, &[], Success::Created), Verdict::Gone);
    assert_eq!(classified(500, &[], Success::Created), Verdict::Other);
    assert_eq!(classified(302, &[], Success::Created), Verdict::Other);
}

#[test]
fn every_operation_judges_every_verdict_as_the_table_states() {
    use github::Action::{Rearm, SkipRepo, SkipRow, Stop, Use};
    use github::Operation::{CloseDeleted, CloseNote, File, Note, Sync};
    use github::Verdict::{
        Accepted, Gone, NotFound, Other, RateLimited, RepoDenied, Unauthorized, Unreachable,
    };
    use github::{Action, Operation, Verdict, action};

    // The expected pair is the recorded policy, one row per operation. The three
    // cells that differ between operations are the ones that always did: a 404 in a
    // sync re-arms the link, a 401 on the note paths costs only its item, and the
    // post-delete close has no Repo skip at all.
    let policy: [(Operation, [(Verdict, Action); 8]); 5] = [
        (
            File,
            [
                (Accepted, Use),
                (RateLimited, Stop),
                (Unauthorized, SkipRepo),
                (RepoDenied, SkipRepo),
                (NotFound, SkipRepo),
                (Gone, SkipRepo),
                (Other, SkipRow),
                (Unreachable, Stop),
            ],
        ),
        (
            Sync,
            [
                (Accepted, Use),
                (RateLimited, Stop),
                (Unauthorized, SkipRepo),
                (RepoDenied, SkipRepo),
                (NotFound, Rearm),
                (Gone, SkipRepo),
                (Other, SkipRow),
                (Unreachable, Stop),
            ],
        ),
        (
            Note,
            [
                (Accepted, Use),
                (RateLimited, Stop),
                (Unauthorized, SkipRow),
                (RepoDenied, SkipRepo),
                (NotFound, SkipRepo),
                (Gone, SkipRepo),
                (Other, SkipRow),
                (Unreachable, Stop),
            ],
        ),
        (
            CloseNote,
            [
                (Accepted, Use),
                (RateLimited, Stop),
                (Unauthorized, SkipRow),
                (RepoDenied, SkipRepo),
                (NotFound, SkipRepo),
                (Gone, SkipRepo),
                (Other, SkipRow),
                (Unreachable, Stop),
            ],
        ),
        (
            CloseDeleted,
            [
                (Accepted, Use),
                (RateLimited, Stop),
                (Unauthorized, SkipRow),
                (RepoDenied, SkipRow),
                (NotFound, SkipRow),
                (Gone, SkipRow),
                (Other, SkipRow),
                (Unreachable, SkipRow),
            ],
        ),
    ];

    let mut stated: Vec<(Operation, Verdict)> = Vec::new();
    for (operation, row) in policy {
        for (verdict, expected) in row {
            assert!(
                !stated.contains(&(operation, verdict)),
                "{operation:?} on {verdict:?} is stated twice"
            );
            stated.push((operation, verdict));
            assert_eq!(
                action(operation, verdict),
                expected,
                "what {operation:?} does with {verdict:?}"
            );
            if verdict == Unreachable {
                // Nothing can act on an answer it never got, which is what lets
                // every loop read the body without a second check.
                assert_ne!(
                    action(operation, verdict),
                    Use,
                    "{operation:?} cannot use an answer it does not have"
                );
            }
        }
    }
    assert_eq!(
        stated.len(),
        40,
        "every operation and verdict pair is stated"
    );
}
