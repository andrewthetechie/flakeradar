---
status: proposed
---

# Read retry attempts from JUnit, and store each attempt as an Execution

When a runner retries a test, today's JUnit uploads usually show only the final outcome, so a test that fails and then passes on retry looks like a clean pass. That hides the strongest flake signal there is. Playwright 1.59 and later can write every failed retry into its JUnit report (`reporter: [['junit', { includeRetries: true }]]`, or `PLAYWRIGHT_JUNIT_INCLUDE_RETRIES=1`). It uses the Maven Surefire elements `<flakyFailure>`, `<flakyError>`, `<rerunFailure>` and `<rerunError>`, with the attempt's message, `time`, `<stackTrace>` and captured output. We now parse these elements. Each failed retry becomes its own failing **Execution** in the same Run, in attempt order, with an attempt number. (This is a test-runner retry inside one Run, not a CI re-run attempt as in ADR 0004.) A test that failed and then passed on retry therefore has a fail and a pass on the same SHA, so the existing rule records a **Proven flake** from the first Run. No scoring change is needed. Proven flakes count on every branch (ADR 0004), so retries on PR branches give evidence too.

## Considered Options

- Our own Playwright reporter: rejected. The built-in JUnit reporter now carries the retry data, and a reporter package is one more thing to publish and version.
- Ingest Playwright's JSON report as a second format: rejected. It needs a second parser and a second upload path for one runner. The Surefire elements also cover Maven, Gradle and other JVM runners at no extra cost.
- Store retries as a count on the final Execution: rejected. The failed attempts have their own Failure message and Failure details, which the UI, the MCP server and failure classification (ADR 0006) need.

## Consequences

- Reports get a `retried` count, and Executions get an `attempt` column (0 for the first attempt).
- Failure counts include failed retries, so a test with retries set to 2 can add 3 failures in one Run. The GitHub `min_failures` gate counts attempts, and its docs must say so.
- Runners that write each attempt as a duplicate `<testcase>` already produce one Execution per attempt. They keep working and get attempt numbers in order of appearance.
- Uploads without these elements behave as before. The CI snippet and README should tell Playwright users to turn on `includeRetries`.
