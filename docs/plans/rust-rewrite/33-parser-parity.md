# 33 — Parser parity: empty namespace, encodings, declarations

Read `00-fixes-2-context.md` first. Findings: `SP2` (must-fix, a task 19 regression), `SP3`, `SP4` (reopen `S10`).

## Goal

The same JUnit bytes give the same Tests, statuses and fingerprints as the reference. A wrong answer here corrupts data silently: a failing test is recorded as passed, or a fingerprint changes and a Test's history splits.

## Contracts

**`SP2`, first and in its own commit.** The reviewer reproduced this on a real server at `ac5e6f3`:

```xml
<testcase classname="c" name="ns"><failure xmlns="" message="boom">x</failure></testcase>
```

It is stored as `last_status: passed`. `xmlns=""` means no namespace, as ElementTree reads it, and `is_tag` at `parsing.rs:546` treats `Some("")` as namespaced. `<testcase xmlns="">` is dropped and a root `<testsuite xmlns="">` answers 422. Fix it for every element the parser matches, and check `attribute` the same way. `<failure xmlns="urn:x">` must stay a non-JUnit element.

**`SP3`.** Follow Python's codecs (task 19's contract). `parsing.rs:292-363` falls through to WHATWG's labels: 224 labels Python accepts answer 422 (`cp437`, `windows_1252`, `iso8859_15`, `latin9`, `utf-8-sig`), 11 go the other way, and 23 decode differently, which changes fingerprints (`iso-8859-9`, `koi8-u`, `tis-620`).

- Generate each single-byte codec's table and the label list from CPython, with the generator script committed under `review-135b/repro/`. Decode through those tables.
- Map multi-byte labels to `encoding_rs` where it decodes as Python does, and prove that per label.
- Labels in the proposed table get its 422.
- A byte a codec leaves undefined follows what the reference does, which you must measure. Replace row `:109`.

**`SP4`.** Read its section. ATTLIST defaults and a BOM plus a declared encoding change fingerprints, parameter entities answer 422, and several malformed forms are accepted. Fix each one that changes a Test's identity or status, or that accepts or refuses differently. The proposed table covers what roxmltree cannot express. Any other case it cannot express is `BLOCKED`, with the case named.

## Done when

- [ ] `SP2`: a test at `parse_junit_xml` and one through `POST /api/ingest` to the stored `last_status`. Red lines are in `STATUS.md`.
- [ ] `SP3`: a committed fixture generated from CPython holds one row per label and byte, and the test reads it. Its failing count before the fix is in `STATUS.md`. Re-running the generator gives the same fixture.
- [ ] `SP4`: one test per case, with the reference's measured answer as a literal.
- [ ] `battery/run.sh all` shows 0 new differences, with `OUT=` outside the repo. Add the `xmlns=""` body to `d5` so the battery covers it from now on.
- [ ] The shared rules are met.
