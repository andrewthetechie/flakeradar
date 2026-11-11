# Flaky-test products compared with FlakeRadar

Research date: 2026-11-10. Sources are the vendors' own documentation pages,
read on that date. A claim with no URL is not verified. Where a page was vague,
this note says so instead of filling the gap.

## 1. Sources

### Snapshot identification

- **Snapshot 3** (`https://pullmd.herrington.services/s/4ccc7d4d`) is a Harness
  blog post about flaky tests. It promotes **Harness CI** (Test Intelligence
  and quarantine). The post gives no algorithm, window or threshold for Harness
  itself. The details below come from Harness's own product page and developer
  docs, which the post links to.
- **Snapshot 4** (`https://pullmd.herrington.services/s/cd222aaf`) is a
  **Trunk** blog post ("the ultimate guide to flaky tests", from its
  `utm_content` links). It describes Trunk Flaky Tests (public beta at the time
  of the post) and links to `docs.trunk.io/flaky-tests`. It adds nothing that
  the Trunk docs do not cover in more detail. **Product 4 is therefore Trunk
  again.** The gap matrix folds it into the Trunk column instead of keeping an
  almost empty fourth column.

### Datadog (Test Optimization, Flaky Tests Management)

- https://docs.datadoghq.com/tests/flaky_tests/
- https://docs.datadoghq.com/tests/flaky_management/
- https://docs.datadoghq.com/tests/flaky_tests/early_flake_detection/
- https://docs.datadoghq.com/tests/flaky_tests/auto_test_retries/
- https://docs.datadoghq.com/tests/developer_workflows/
- https://docs.datadoghq.com/pr_gates/
- https://docs.datadoghq.com/tests/setup/junit_xml/
- https://docs.datadoghq.com/tests/ (supported-features table)
- https://docs.datadoghq.com/tests/test_impact_analysis/
- https://docs.datadoghq.com/bits_ai/mcp_server/
- https://docs.datadoghq.com/mcp_server/tools/ (read in two parts; the last
  ~20 KB of the page was not read)

### Trunk Flaky Tests

- https://docs.trunk.io/flaky-tests/detection
- https://docs.trunk.io/flaky-tests/detection/pass-on-retry-monitor
- https://docs.trunk.io/flaky-tests/detection/failure-rate-monitor
- https://docs.trunk.io/flaky-tests/detection/failure-count-monitor
- https://docs.trunk.io/flaky-tests/detection/new-test-monitor
- https://docs.trunk.io/flaky-tests/detection/skipped-test-monitor
- https://docs.trunk.io/flaky-tests/detection/slow-test-monitor
- https://docs.trunk.io/flaky-tests/detection/timeout-inflation-monitor
- https://docs.trunk.io/flaky-tests/detection/flag-as-flaky
- https://docs.trunk.io/flaky-tests/detection/infrastructure-failure-protection
- https://docs.trunk.io/flaky-tests/detection/the-importance-of-pr-test-results
- https://docs.trunk.io/flaky-tests/quarantining
- https://docs.trunk.io/flaky-tests/quarantining/quarantine-service-availability
- https://docs.trunk.io/flaky-tests/management/github-pull-request-comments
- https://docs.trunk.io/flaky-tests/management/ticketing
- https://docs.trunk.io/flaky-tests/management/ticketing/automatic-ticketing
- https://docs.trunk.io/flaky-tests/management/managing-detected-flaky-tests
- https://docs.trunk.io/flaky-tests/management/test-labels
- https://docs.trunk.io/flaky-tests/webhooks
- https://docs.trunk.io/flaky-tests/webhooks/slack-integration
- https://docs.trunk.io/flaky-tests/webhooks/github-issues-integration
- https://docs.trunk.io/flaky-tests/reference/mcp-reference
- https://docs.trunk.io/flaky-tests/reference/mcp-reference/fix-flaky-test
- https://docs.trunk.io/flaky-tests/reference/mcp-reference/investigate-ci-failure
- https://docs.trunk.io/flaky-tests/agents/autofix-flaky-tests
- https://docs.trunk.io/flaky-tests/agents/autofix-ci-failures
- https://docs.trunk.io/flaky-tests/reference/cli-reference
- https://docs.trunk.io/flaky-tests/dashboard
- https://docs.trunk.io/flaky-tests/test-collections
- https://docs.trunk.io/flaky-tests/get-started/multiple-repositories
- https://docs.trunk.io/flaky-tests/overview
- https://docs.trunk.io/llms.txt (page index)
- Not readable: https://docs.trunk.io/references/apis/flaky-tests redirects to a
  login page. **The Trunk Flaky Tests REST API endpoints are not verified.**

### Harness CI (snapshot 3)

- https://pullmd.herrington.services/s/4ccc7d4d (the blog snapshot)
- https://www.harness.io/products/continuous-integration/test-intelligence
- https://developer.harness.io/docs/continuous-integration/use-ci/run-tests/test-management/ci-test-management-overview/
- https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-flaky-tests.md
- https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-policies.md
- https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-quarantine.md
- https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-reports.md
- https://developer.harness.io/docs/continuous-integration/use-ci/run-tests/tests-v2/

### Trunk blog (snapshot 4)

- https://pullmd.herrington.services/s/cd222aaf

## 2. Per-product feature summary

### 2.1 Datadog Test Optimization / Flaky Tests Management

**Detection.** A flaky test is one that "passes and fails across multiple runs
of the same commit". Runs carry the tags `is_flaky`, `is_new_flaky` (first time
flaky on the current or default branch) and `is_known_flaky` (a failure of a
test already known flaky on the current or default branch).
[flaky_tests](https://docs.datadoghq.com/tests/flaky_tests/)

- Branches: the current branch and the default branch. The default-branch table
  also takes flaky tests from feature branches merged into it, found by walking
  the last 5,000 commits. Squash, reset or force-push can hide a branch's
  flakes. [flaky_tests](https://docs.datadoghq.com/tests/flaky_tests/)
- A flaky test with no failure in 30 days drops off the table automatically. A
  user can remove one by hand, and "Ignore flaky tests" removes false new-flaky
  detections for one commit. Either comes back if the test flakes again.
  [flaky_tests](https://docs.datadoghq.com/tests/flaky_tests/)
- The table ranks by commits flaked (up to 1,000 tests), with average duration,
  first/last flaked, failure rate since first flake and a trend line. A metric
  counts default-branch flaky tests every 30 minutes.
  [flaky_tests](https://docs.datadoghq.com/tests/flaky_tests/)

**States.** `Active`, `Quarantined` (runs, failures do not affect CI status),
`Disabled` (skipped entirely, so no new data) and `Fixed`.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)
Quarantine and disable need a minimum version of Datadog's native test library
for each language (for example Python `3.3.0+`, JavaScript `5.44.0+`). The
docs do not say whether a JUnit XML upload can be quarantined; the
supported-features table has no quarantine row.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/),
[tests](https://docs.datadoghq.com/tests/)

**Policies (automation), per repository or as org defaults.**
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)

- Quarantine rules: after N days `Active` (evaluated daily 12:15 UTC); when the
  test flakes on named branches; when the failure rate is at or above a
  threshold over the last 7 days (every 15 minutes).
- Disable rules: after N days in a state (daily 12:30 UTC); by branch; by 7-day
  failure rate.
- `Fixed` is automatic and not configurable: 30 days with no flake. A test with a
  100% failure rate over 7 days is not auto-fixed and is filterable as
  `broken_test:true`.
- Metric `test_optimization.test_management.flaky_tests`, tagged by repository,
  service, branch, status, codeowners and flaky category.

**Fix verification ("Attempt to Fix").** Put the test key (for example
`DD_ABC123`) in a commit message. The library retries the test 20 times by
default on that commit. All pass: the fix is tracked until the branch merges,
then a 14-day grace period treats the test as quarantined. Any fail: state is
kept. Needs Source Code Integration (GitHub, GitLab or Azure DevOps) for merge
detection; fixes older than three months stop being tracked.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)

**Early Flake Detection (EFD).** The library fetches the list of known tests
before a session. A test not in the list is new and is retried up to 10 times;
any failing attempt tags it flaky. Tests slower than 5 minutes are excluded. A
test inactive more than 14 days may count as new again. Enabled per org,
repository or service. Pairs with a PR Gate to block merges that add new flaky
tests.
[early_flake_detection](https://docs.datadoghq.com/tests/flaky_tests/early_flake_detection/)
The page does not name branches that EFD applies to.

**Auto Test Retries (ATR).** The library retries any failing test up to 5 times
(`DD_CIVISIBILITY_FLAKY_RETRY_COUNT`), with a session cap of 1000 retried tests
(`DD_CIVISIBILITY_TOTAL_FLAKY_RETRY_COUNT`) in most languages. Python has a
"dynamic" mode that sets the budget from first-attempt duration (10 retries at
5 s or less, down to 1 above 5 minutes). Java can retry only known flakes.
[auto_test_retries](https://docs.datadoghq.com/tests/flaky_tests/auto_test_retries/)

**JUnit XML upload.** `datadog-ci junit upload` is supported, but Datadog
"recommends the native instrumentation". The supported-features table marks
EFD, ATR and Test Impact Analysis "yes" for JUnit XML; it is not explained how
a post-hoc upload retries anything, so treat that as unverified. Code owners for
JUnit uploads come from CODEOWNERS through the GitHub integration, and are
"partially" supported.
[junit_xml](https://docs.datadoghq.com/tests/setup/junit_xml/),
[tests](https://docs.datadoghq.com/tests/)

**AI.** Every flaky test gets an AI root-cause category (Concurrency,
Randomness, Floating Point, Unordered Collection, Too Restrictive Range,
Timeout, Order Dependency, Resource Leak, Asynchronous Wait, IO, Network, Time,
Environment Dependency, Unknown). "Bits Code" can generate a fix PR for a test
with failure rate ≥ 5%, wasted time ≥ 2 h, ≥ 2 failed pipelines, a flake on the
default branch, and an error message plus source file.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)

**Ownership.** Owners come from CODEOWNERS reported in default-branch runs.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)

**Notifications.** Events: new flaky test on the default branch, quarantine or
disable by policy, manual state change, fix succeeded, fix failed, and a weekly
digest (beta). Channels: email or Slack. Rules route by code owner.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)
CI Test monitors can alert on test search queries.
[tests](https://docs.datadoghq.com/tests/)

**PR integration.** GitHub-only PR comments with test results, flakiness and
error messages. Pre-filled GitHub issues and Jira issues (via Work Management,
with status sync back). "View Code" opens VS Code, IntelliJ or GitHub.
[developer_workflows](https://docs.datadoghq.com/tests/developer_workflows/)
PR Gates can block a PR that "introduces at least one new flaky test"; they act
on GitHub, GitLab, Azure DevOps and Bitbucket Cloud, block only violations the
PR itself introduced, and are not supported for fork PRs or on GitHub merge
queues. [pr_gates](https://docs.datadoghq.com/pr_gates/)

**Test Impact Analysis.** Coverage-based test selection that skips tests a
change cannot affect. [test_impact_analysis](https://docs.datadoghq.com/tests/test_impact_analysis/)

**API and MCP.** A Flaky Tests Management API identifies tests by `id`,
filterable by `fingerprint_fqn`.
[flaky_management](https://docs.datadoghq.com/tests/flaky_management/)
The Datadog MCP server's "Software Delivery" toolset has
`get_datadog_flaky_tests`, `search_datadog_test_events`,
`aggregate_datadog_test_events`, CI pipeline search and aggregate tools,
`get_datadog_test_optimization_settings`,
`get_datadog_flaky_tests_management_policies`, and one write tool:
`update_datadog_flaky_test_states` (quarantine, disable or mark fixed;
"requires explicit user approval").
[mcp_server/tools](https://docs.datadoghq.com/mcp_server/tools/)

### 2.2 Trunk Flaky Tests

**Detection = independent monitors.** Each monitor is active or inactive per
test. When it activates it runs its action; when it resolves the action is
undone. Detection runs on upload processing, within at most 20 minutes.
[detection](https://docs.trunk.io/flaky-tests/detection)

| Monitor | Kind | How it works |
|---|---|---|
| Pass-on-retry | classifies (flaky only) | A fail and a pass on the same commit, on any branch by default; the two can arrive hours or days apart. Resolves after a recovery period with no new occurrence (default 7 days, range 1–15, checked daily). Option: minimum count of distinct commits (default 1). [pass-on-retry](https://docs.trunk.io/flaky-tests/detection/pass-on-retry-monitor) |
| Failure rate | classifies flaky or broken | Rate over a rolling window in hours, with a minimum sample size, separate activation and resolution thresholds (hysteresis), optional stale timeout, and up to 10 branch globs. Recommended: broken on main 80–100%; flaky on main 20–50% over 12–72 h; merge queue 10–15% (resolve 5%) over 6–12 h. Advises against flake-rate analysis on PR branches. [failure-rate](https://docs.trunk.io/flaky-tests/detection/failure-rate-monitor) |
| Failure count | classifies flaky or broken | N failures in a rolling window on named branches (default N = 1); resolves after a resolution timeout with no new failure. Meant for branches where any failure is unexpected. [failure-count](https://docs.trunk.io/flaky-tests/detection/failure-count-monitor) |
| New test | label only | Labels a test for `newDays` after its first upload. [new-test](https://docs.trunk.io/flaky-tests/detection/new-test-monitor) |
| Skipped test | label only | N skips in a window. [skipped-test](https://docs.trunk.io/flaky-tests/detection/skipped-test-monitor) |
| Slow test | label only, off by default | A duration percentile over a window at or above a fixed ms threshold. [slow-test](https://docs.trunk.io/flaky-tests/detection/slow-test-monitor) |
| Timeout inflation | label only, off by default | Failing p50 ≥ 2× passing p95 and gap > 3000 ms over 168 h (≥ 5 passes, ≥ 3 fails); resolves at ratio ≤ 1.5. [timeout-inflation](https://docs.trunk.io/flaky-tests/detection/timeout-inflation-monitor) |

- Status is the most severe active classification: Broken > Flaky > Healthy.
  [detection](https://docs.trunk.io/flaky-tests/detection)
- A new test collection starts with: pass-on-retry (all branches, 7-day
  resolve) → flaky; failure count 5 → flaky; failure rate 90% over 24 h with 5
  samples → broken; new-test and skipped-test labels. All but pass-on-retry
  watch `main`, `master`, `develop`, `dev` and `release/*`.
  [test-collections](https://docs.trunk.io/flaky-tests/test-collections)
- Mute: a monitor can be muted for one test for 1 h, 4 h, 24 h, 7 d or 30 d.
  Any monitor can switch to "Apply labels" as a dry run.
  [detection](https://docs.trunk.io/flaky-tests/detection); "Forever" is also
  listed in
  [managing](https://docs.trunk.io/flaky-tests/management/managing-detected-flaky-tests).
- Flag as Flaky: a manual flag with a reason (256 chars), with flag history; it
  cannot override Broken.
  [flag-as-flaky](https://docs.trunk.io/flaky-tests/detection/flag-as-flaky)
- Infrastructure failure protection: an upload whose failing share exceeds a
  threshold (default 80%) is excluded from detection and metrics but still
  listed.
  [infrastructure-failure-protection](https://docs.trunk.io/flaky-tests/detection/infrastructure-failure-protection)
- PR runs are the main source of same-commit retries and of the "PRs impacted"
  metric.
  [importance-of-pr-test-results](https://docs.trunk.io/flaky-tests/detection/the-importance-of-pr-test-results)

**Quarantine = run, then override the exit code.** The CLI reads JUnit results,
asks Trunk which failed tests are quarantined, and exits 0 when every failure is
quarantined. Tests still run and upload. Auto-quarantine applies to Flaky tests
only (not Broken) and is off by default. Manual overrides are "Always
Quarantine" and "Never Quarantine", each with a required comment, gated by an
"Anyone / Admins only" permission. Events tab and audit logs record changes.
[quarantining](https://docs.trunk.io/flaky-tests/quarantining)
It fails closed: if the CLI cannot reach the service, no test is quarantined and
the original exit code stands; there is no local cache.
[quarantine-service-availability](https://docs.trunk.io/flaky-tests/quarantining/quarantine-service-availability)
The page does not say whether an auto-quarantined test is released when it
recovers. **Not verified.**

**Uploader.** `trunk-analytics-cli upload | test | validate`. Formats: JUnit XML,
Bazel BEP JSON, XCResult, Swift xunit. `--variant` separates the same test by
environment. Fork PR uploads are opt-in.
[cli-reference](https://docs.trunk.io/flaky-tests/reference/cli-reference),
[multiple-repositories](https://docs.trunk.io/flaky-tests/get-started/multiple-repositories)
Test collections group tests with their own detection, quarantine and ticketing
settings, and are many-to-many with repositories.
[test-collections](https://docs.trunk.io/flaky-tests/test-collections)

**PR comments.** GitHub only; posted when there are failing tests; lists passed,
failed, flaked, skipped and quarantined tests, with failure history and stack
traces. Not supported for test collections.
[github-pull-request-comments](https://docs.trunk.io/flaky-tests/management/github-pull-request-comments)

**Ticketing.** Built-in Jira (Cloud and Data Center), Linear and Asana. Ticket
body: test id, first flaky, last failure, PR and branch impact, quarantine
status, common failure reasons, code owners.
[ticketing](https://docs.trunk.io/flaky-tests/management/ticketing)
Automatic ticketing per collection: create on → flaky and/or → broken; one
ticket per test; escalation edits the description in place; auto-close on
healthy; reopen within a window (default 30 days) or file new.
[automatic-ticketing](https://docs.trunk.io/flaky-tests/management/ticketing/automatic-ticketing)
GitHub Issues only through a webhook transformation, with no lifecycle
management.
[github-issues-integration](https://docs.trunk.io/flaky-tests/webhooks/github-issues-integration)

**Webhooks.** Delivered by Svix, with retry and backoff. Events:
`test_case.monitor_status_changed`, `v2.test_case.status_changed`,
`test_case.investigation_completed`, and (from the quarantine page)
`test_case.quarantining_setting_changed`. Payloads include code owners and
quarantine state. Request signing is not described on the page.
[webhooks](https://docs.trunk.io/flaky-tests/webhooks),
[quarantining](https://docs.trunk.io/flaky-tests/quarantining)
Slack and Teams messages are Svix JavaScript transformations.
[slack-integration](https://docs.trunk.io/flaky-tests/webhooks/slack-integration)

**Dashboard.** Flaky and broken counts, "PRs impacted", daily charts of new
tests, status changes (newly flaky, newly broken, recovered) and quarantined
runs; top-10 most unstable, longest-running and longest-quarantined tables;
filters by branch, code owner and repository. Test detail: failures grouped by
"failure type" with an optional AI summary, run history with branch globs and
SHA filter, a 30-day monitor swimlane, and an events timeline. Inactive tests
leave after 30 days and are removed after 45.
[dashboard](https://docs.trunk.io/flaky-tests/dashboard)
The overview also names "PRs rescued by quarantining" and a Cmd+K palette.
[overview](https://docs.trunk.io/flaky-tests/overview)

**Labels.** Org-scoped labels, manual or applied by a monitor, with provenance.
[test-labels](https://docs.trunk.io/flaky-tests/management/test-labels)

**AI and MCP.** MCP at `https://mcp.trunk.io/mcp`, OAuth or org bearer token.
Tools: `search-test`, `list-test-collections`, `fix-flaky-test`,
`investigate-ci-failure`, `setup-trunk-uploads`. None changes test status or
quarantine. [mcp-reference](https://docs.trunk.io/flaky-tests/reference/mcp-reference)
`fix-flaky-test` returns an investigation (summary and facts) and can start a
new one (counts against quotas).
[fix-flaky-test](https://docs.trunk.io/flaky-tests/reference/mcp-reference/fix-flaky-test)
`investigate-ci-failure` takes a GitHub Actions run URL and returns failures
with quarantined tests filtered out.
[investigate-ci-failure](https://docs.trunk.io/flaky-tests/reference/mcp-reference/investigate-ci-failure)
Autofix (beta): an investigation starts on each move from Healthy to Flaky or
Broken (100 per repo and 5 per test each month); the PR itself comes from a
Cursor Automation driven by the webhook, not from Trunk.
[autofix-flaky-tests](https://docs.trunk.io/flaky-tests/agents/autofix-flaky-tests),
[autofix-ci-failures](https://docs.trunk.io/flaky-tests/agents/autofix-ci-failures)

**API.** The overview and quarantine pages mention a Flaky Tests API that lists
quarantined tests. The API reference is behind a login. **Endpoints not
verified.** [quarantining](https://docs.trunk.io/flaky-tests/quarantining)

### 2.3 Harness CI Test Management (snapshot 3)

The blog makes marketing claims (ML detection of load, parallel-only or
OS-specific flakes; root-cause suggestions; flaky-rate dashboards across teams)
without mechanism.
[snapshot 3](https://pullmd.herrington.services/s/4ccc7d4d) The developer docs
are narrower and are what this note relies on:

- **Detection:** a test is flaky when it passed and failed on the same commit
  within a 14-day window, per repository. No rate threshold. It clears after 5
  consecutive passes, or after 14 days with no same-commit inconsistency. Branch
  behaviour is not described. Manual tri-state override: force flaky, force
  stable, or unset.
  [ci-flaky-tests](https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-flaky-tests.md)
- **Policies:** up to 10 JSON `when`/`action` rules per repo, evaluated in order
  after each pipeline execution. Conditions: `test failed`, `test is flaky`,
  `test is slow` (threshold not stated). Actions: mark/unmark flaky, mark
  unflaky, mark/unmark quarantine. Feature flag `TI_POLICY_EVALUATION_ENABLED`.
  [ci-test-policies](https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-policies.md)
- **Quarantine:** quarantined tests run and are recorded, but Harness ignores
  their failures when it sets the step status. Manual via
  `hcli test-management set --quarantine=true`, or by policy. No automatic
  expiry; the doc recommends "7 days max" and a tracking issue.
  [ci-test-quarantine](https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-quarantine.md)
- **Ingest:** `hcli test-reports upload` with JUnit XML.
  [ci-test-reports](https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-test-reports.md)
- **UI:** a FLAKY badge and filter on the pipeline execution's Tests tab. The
  docs describe no UI for quarantine.
  [ci-flaky-tests](https://developer.harness.io/continuous-integration/use-harness-ci/use-harness-ci/run-tests/test-management/ci-flaky-tests.md)
- **Test Intelligence:** test selection from code changes, plus test splitting.
  [tests-v2](https://developer.harness.io/docs/continuous-integration/use-ci/run-tests/tests-v2/),
  [product page](https://www.harness.io/products/continuous-integration/test-intelligence)
- The product page shows per-test "flake rate, failure history, and last
  known-good run" and a monthly quarantine summary.
  [product page](https://www.harness.io/products/continuous-integration/test-intelligence)
  The docs read do not show where this lives. **Not verified.**
- Not described in the docs read: notifications, ticketing, REST API, MCP.

### 2.4 Trunk blog (snapshot 4)

Restates Trunk features at a high level: automatic detection, quarantine that
keeps tests running without blocking merges, impact measured as "PRs affected",
and GitHub PR comments. No algorithm or threshold.
[snapshot 4](https://pullmd.herrington.services/s/cd222aaf)

## 3. FlakeRadar current features

- **Data model:** Repo (`owner/name`) → Project (suite) → Test; Pipeline → Job;
  CI run and CI run attempt; Report → Run → Execution. `CONTEXT.md`,
  `docs/adr/0001-repo-project-split.md`.
- **Ingest:** JUnit XML via `POST /api/ingest`, answered `202` and processed by
  a single leader on a Postgres queue; duplicate-upload detection; failed
  reports kept for retry or dismissal. `docs/api.md`,
  `docs/ci-integration.md`, `docs/adr/0002-queued-ingest.md`. GitHub report
  action `actions/report`; `curl` for other CIs; GitLab job reporter
  (`samples/gitlab`, `docs/ci-integration.md#gitlab`).
- **Retries inside a run:** Surefire `flakyFailure`/`rerunFailure`, Playwright
  `includeRetries`, nextest (including `flaky-result = "fail"`); each attempt is
  an Execution. `docs/adr/0005-retry-attempts-from-junit.md`.
- **Scoring:** weighted flip rate over the last 50 executions (decay 0.85),
  damping below 7 executions, same-SHA "proven flake" floor of 0.6 (+0.1 each,
  max 1.0). Flips count only on the default branch; proven flakes count on all
  branches. Tiers flaky (≥ 0.30) / suspect / stable. One global threshold,
  window and decay per instance. `docs/concepts.md`, `docs/configuration.md`,
  `docs/adr/0004-pushed-ci-jobs-and-attribution.md`.
- **Broken tests:** `fail_streak` ≥ 5 on the default branch sets a `broken`
  flag (`?broken=1`). `docs/concepts.md#broken-tests`.
- **History:** daily score history (365 days), trend vs 14 days ago, clean
  streak, "new flaky in N days". `docs/adr/0007-daily-score-history.md`.
- **Failure categories:** five rule-based categories (network, environment,
  timing, assertion, other); LLM classification rejected.
  `docs/adr/0006-rule-based-failure-categories.md`.
- **Location:** file, line and GitHub permalink at the last failing commit;
  full traceback. `docs/ci-integration.md#test-locations`.
- **CI jobs:** job flakiness scored from unexplained failures only; summary
  (alls-green) jobs; failures grouped by runner label set and runner name.
  `docs/adr/0004-pushed-ci-jobs-and-attribution.md`,
  `docs/ci-integration.md#failures-by-runner`.
- **CI run failures:** `GET /api/ci-runs/{id}/failures` and MCP
  `ci_run_failures`: failing tests with tier, score, proven flakes, quarantine
  and "passed on retry", and failed jobs marked explained or unexplained. The
  summary action writes this to the GitHub job summary and never fails the job.
  `docs/ci-integration.md#failures-of-one-ci-run`.
- **Quarantine:** a human toggle in the dashboard with optional reason and
  expiry (never, 7, 14, 30 days); `GET /api/quarantine` gives the runner a
  **skip** list; maintenance clears expired quarantines; never automatic; not
  in MCP. `docs/concepts.md#quarantine`, `CONTEXT.md`. Runner-side consumers
  are planned in open issues #139–#145. The toggle endpoint has no auth
  (`docs/api.md`).
- **GitHub issues:** automatic filing behind a gate (min score, min proven
  flakes, min failures); dedupe by issue link; sync clears the link on
  completed/deleted and keeps it on `not_planned`; optional "fix appears to
  hold" comment and close at a clean streak; links from other filers via API
  and MCP. `docs/github-issues.md`,
  `docs/adr/0008-issue-links-from-other-filers.md`.
- **Notifications:** `test.flaky`, `test.recovered`, `job.flaky`,
  `job.recovered` on threshold crossings; JSON or Slack text; best effort, no
  outbox. `docs/notifications.md`, `docs/adr/0009-webhook-notifications.md`.
- **MCP:** 11 tools, 9 read and 2 write (`link_issue`, `unlink_issue`); "quarantine
  stays a human decision". `docs/mcp.md`.
- **Dashboard:** repo/project picker, paginated leaderboards for tests and
  jobs, filters (category, quarantined, has issue, new, file, query), detail
  panel with sparkline, CI run view, shareable URLs. `README.md`,
  `frontend/src/App.tsx`.
- **Ops:** OpenAPI and Redoc, health and readiness endpoints, Prometheus gauges,
  retention and admin deletes. `docs/api.md`, `docs/monitoring.md`,
  `docs/configuration.md`.
- Durations are parsed and stored per Execution and returned in history
  (`crates/flakeradar-core/src/parsing.rs`, `crates/flakeradar-core/src/dto.rs`),
  but nothing analyses them.
- `test_cases` has no first-seen timestamp and no owner column
  (`crates/flakeradar-db/migrations/0001_baseline.sql`).

## 4. Gap matrix

"yes" means the docs read describe it; "partial" is qualified in Notes; "no"
means the docs read do not describe it. Trunk includes snapshot 4.

| Feature | Datadog | Trunk | Harness (snap. 3) | FlakeRadar | Notes |
|---|---|---|---|---|---|
| Same-commit pass+fail detection | yes | yes | yes | yes | FlakeRadar: proven flake floor on all branches. |
| Rate or score over a window | partial (failure rate in policies) | yes (rate, count) | no | yes | FlakeRadar: flip score, default branch only. |
| Broken (consistently failing) state | partial (`broken_test` filter) | yes | no | yes | FlakeRadar: flag at fail streak 5. |
| Configurable rules per repo/suite | yes (policies) | yes (monitors per collection) | yes (policies) | no | FlakeRadar settings are instance-wide. |
| Activation/resolution hysteresis | no | yes | partial (5 passes or 14 d) | no | FlakeRadar notifies on each threshold crossing. |
| Branch-scoped rules / merge-queue branches | yes | yes (globs) | no | partial | FlakeRadar: fixed rule, default branch vs others. |
| Mute / ignore a detection | partial (ignore per commit) | yes (1 h to 30 d, forever) | no | partial | FlakeRadar: `not_planned` issue silences filing only. |
| Manual flag flaky / force stable | no | yes (flag) | yes (tri-state) | no | ADR 0007 deferred "mark as fixed". |
| Quarantine: run, ignore failure | yes | yes (CLI exit code) | yes | no | FlakeRadar's quarantine is a skip list. |
| Quarantine: skip (disable) | yes | no | no | yes | Matches Datadog "Disabled". |
| Automatic quarantine | yes (policy) | yes (opt-in) | yes (policy) | no | **Conflicts with FlakeRadar design.** |
| Quarantine expiry | partial (time-based rules) | no | no (advice only) | yes | FlakeRadar: 7/14/30 days, then cleared. |
| Quarantine enforcement tool for the runner | yes (libraries) | yes (CLI) | yes (step status) | partial | FlakeRadar: list API only; consumers in #139–#145. |
| Fail-safe when service is down | not described | yes (fail closed) | not described | n/a | Becomes relevant once FlakeRadar gates CI. |
| Fix verification | yes (Attempt to Fix, 20 retries) | no | no | partial | FlakeRadar: clean streak, fixed-note comment, optional close. |
| "Fixed" state | yes (30 d) | yes (healthy on resolve) | yes (auto clear) | partial | FlakeRadar: score decays; no state. |
| New-test detection / EFD | yes (EFD retries new tests) | partial (label only) | no | no | No first-seen column. |
| Runner auto-retries | yes (library) | no | no | no | FlakeRadar ingests retries; it does not drive them. |
| Infra / mass-failure exclusion | no | yes (80% default) | no | partial | FlakeRadar: job attribution and runner grouping; no per-upload exclusion. |
| Environment variants (OS, matrix) | yes (configuration tags) | yes (`--variant`) | no | partial | FlakeRadar: separate projects only; job matrix legs are separate Jobs. |
| Code owners | yes | yes | no | no | |
| Labels / tags | yes (tags) | yes | no | no | |
| Slow / duration analysis | partial (duration column) | yes (slow, timeout inflation) | partial (`test is slow`) | no | Durations already stored. |
| Skipped-test tracking | no | yes (label) | no | no | |
| Impact metric (PRs/runs affected, wasted time) | yes (wasted time, pipelines) | yes (PRs impacted) | no | partial | FlakeRadar: counts of executions and failures. |
| Failure grouping within a test | no | yes (failure type) | no | partial | FlakeRadar: category per execution. |
| Root-cause category | yes (AI, 14 kinds) | partial (AI summary) | no | yes (rules, 5 kinds) | ADR 0006 rejects LLM classification. |
| Server-side AI investigation / fix PR | yes (Bits) | yes (beta) | no (blog only) | no | **Conflicts with ADR 0006 reasoning.** |
| PR comment | yes (GitHub) | yes (GitHub) | no | no | FlakeRadar: job summary only. |
| PR gate on new flaky tests | yes | no | no | partial | `ci_run_failures` lets a gate decide; no shipped gate. |
| GitHub issue filing | partial (pre-filled, manual) | partial (webhook transform) | no | yes | FlakeRadar: automatic, gated, synced. |
| Jira / Linear / Asana | yes (Jira) | yes (all three) | no | no | |
| Ticket reopen on regression | no | yes | no | no | FlakeRadar files a new issue after a completed close. |
| Slack | yes | yes | no | yes | |
| Teams / email | yes (email) | yes (Teams) | no | no | |
| Owner-routed notifications | yes | partial (transform) | no | no | Needs code owners first. |
| Broken / escalation events | no | yes (status changed) | no | no | ADR 0009: broken sends nothing. |
| Weekly digest | yes (beta) | partial (on request) | no | no | |
| Guaranteed webhook delivery | not described | yes (Svix retries) | n/a | no | ADR 0009 names an outbox as the first change. |
| Audit / event timeline | partial (state changes notified) | yes | no | no | FlakeRadar has no user identity. |
| Permissions on quarantine | not described | yes | partial (PAT) | no | Quarantine toggle is unauthenticated by design. |
| Aggregate trend charts | yes (metric, dashboards) | yes | partial | partial | FlakeRadar: Prometheus tier gauges; per-test sparkline. |
| MCP read tools | yes | yes | no | yes | |
| MCP write: quarantine/state | yes (with approval) | no | no | no | **Conflicts with FlakeRadar design.** |
| MCP: CI run failures | partial (event search) | yes | no | yes | |
| MCP: settings / policies read | yes | no | no | no | |
| MCP: setup help | yes (onboarding) | yes | no | no | |
| CI job flakiness | not verified | no | no | yes | See section 6. |
| Non-JUnit formats | yes (native libraries) | yes (Bazel, XCResult) | partial (TRX) | no | |
| Test selection (TIA) | yes | no | yes | no | Out of scope for FlakeRadar. |
| Fork PR uploads | no (PR Gates) | yes | no | no | |
| Self-hosted | no | no | partial (Harness SMP not checked) | yes | |

## 5. Missing features in FlakeRadar

Sizes: S is days, M is about a week or one PR with migrations, L is several PRs
or a model change.

### 5.1 Quarantine that runs the test and ignores its failure — M

- **Competitors:** Datadog Quarantined state; Trunk CLI exit-code override;
  Harness ignores quarantined failures in the step status. All three keep the
  test running, so evidence keeps arriving.
- **FlakeRadar today:** quarantine means skip. A skipped test sends no new
  executions, so its clean streak cannot grow and the fixed-note comment can
  never fire for it. The "did the fix hold?" story breaks for quarantined tests.
- **Mapping:** a "gate" mode in the summary action, or a new `actions/gate`:
  after the reports of the attempt are processed, read
  `/api/ci-runs/{id}/failures?ci_job_id=...` and exit non-zero only when a
  failure is not quarantined. The test step runs with `continue-on-error`. Most
  of the data path exists (`quarantined` is already in each failure entry).
- **Open questions:**
  - Fail open or fail closed when FlakeRadar is down or `reports.pending`
    stays above 0? Trunk fails closed.
  - Does a skip list stay as a second mode (Datadog has both Quarantined and
    Disabled)?
  - The gate and CI need the upload to finish first. How long can a job wait?

### 5.2 Automatic quarantine and policy rules — L, conflicts with design

- **Competitors:** Datadog policies quarantine or disable by days active, branch
  or 7-day failure rate. Trunk auto-quarantines Flaky (not Broken) tests when
  the setting is on. Harness quarantines by JSON policy.
- **Conflict:** `CONTEXT.md` (Quarantine) says "FlakeRadar never quarantines a
  Test on its own", and the MCP instructions say "quarantine stays a human
  decision". **Do not build this without a decision that changes that rule.**
- A middle ground that keeps the rule: "suggested quarantine" — a leaderboard
  filter or notification that proposes candidates, and a person confirms.
- **Open question:** keep the rule, or allow opt-in auto-quarantine per repo
  with a required expiry?

### 5.3 Per-repo or per-project detection settings — L

- **Competitors:** Trunk monitors per test collection, each with branch globs,
  windows, sample minimums and separate activation and resolution thresholds.
  Datadog policies per repository with org defaults.
- **FlakeRadar today:** `FLAKE_THRESHOLD`, `SCORE_WINDOW`, `SCORE_DECAY` and
  `BROKEN_STREAK` are instance-wide (`docs/configuration.md`). An e2e suite and
  a unit suite share one threshold.
- **Mapping:** nullable overrides on `projects` (and `repos` for jobs), read by
  rescore. Changing an override needs a rescore of that project. Score history
  shows a step, as ADR 0007 already accepts for rule changes.
- **Open questions:** which settings may vary? Does the `/metrics` tier gauge
  use each project's threshold?

### 5.4 Hysteresis on tier changes — S

- **Competitors:** Trunk's resolution threshold below the activation threshold,
  and recovery periods (pass-on-retry clears after 7 days).
- **FlakeRadar today:** a score that hovers near 0.30 sends `test.flaky` and
  `test.recovered` repeatedly (`docs/notifications.md`).
- **Mapping:** a `RECOVER_THRESHOLD` (default equal to `FLAKE_THRESHOLD`) used
  only when a Test is already flaky. Rescore already returns crossings
  (ADR 0009).
- **Open question:** apply hysteresis to the tier itself, or only to
  notifications?

### 5.5 Mute or acknowledge a flaky test — S/M

- **Competitors:** Trunk mutes one monitor for one test for 1 h to 30 d or
  forever. Datadog ignores false new-flaky detections per commit.
- **FlakeRadar today:** an issue closed as `not_planned` keeps the filer quiet,
  but the test stays on the leaderboard and still notifies.
- **Mapping:** an `acknowledged_until` and reason on `test_cases` and `jobs`;
  filters on the leaderboard and API; no notification while acknowledged. This
  is not a quarantine: the runner still runs and fails it.
- **Open question:** is acknowledge a human-only action, like quarantine, or
  may `link_issue`-style MCP write it?

### 5.6 Manual flag / force-stable override — S

- **Competitors:** Trunk Flag as Flaky (with history); Harness tri-state
  force flaky / force stable / unset.
- **FlakeRadar today:** none. ADR 0007 deferred a manual "mark as fixed".
- **Open question:** does an override change the tier (and the filer and
  notifications), or only add a badge?

### 5.7 New-test detection and Early Flake Detection — S (label) / L (EFD)

- **Competitors:** Trunk labels a new test for N days. Datadog EFD retries new
  tests up to 10 times inside the runner and can gate the PR.
- **FlakeRadar today:** no first-seen column on `test_cases`.
- **Mapping (S):** add `first_seen_at`, a `new_test` filter, and show "new" on
  `ci_run_failures` entries so a gate can treat a failing new test differently.
- **Mapping (L):** a `GET /api/known-tests?repo&project` endpoint plus runner
  samples that repeat unknown tests. This needs runner code for each framework,
  like the quarantine consumers in #139–#145.
- **Open question:** is runner-side tooling in scope? Today FlakeRadar is
  push-only from CI and owns no runner plugin.

### 5.8 Fix verification and a "Fixed" state — M

- **Competitors:** Datadog Attempt to Fix (commit key, 20 retries on the fix
  commit, a 14-day grace period after merge, `Fixed` after 30 days with no
  flake).
- **FlakeRadar today:** clean streak, a falling score, and an optional
  "the fix appears to hold" comment and close (`GITHUB_FIXED_STREAK`,
  `GITHUB_CLOSE_FIXED`). That covers the after-merge half.
- **Mapping:** a sample workflow that repeats one test N times on a PR and
  uploads each attempt; `ci_run_failures` then shows all-pass or not. No server
  change needed for a first version.
- **Open question:** is a stored "fixed" state wanted, or are clean streak and
  score enough (ADR 0007 chose the latter)?

### 5.9 Infrastructure failure protection for JUnit uploads — S/M

- **Competitors:** Trunk drops an upload from detection when more than 80% of
  its tests fail.
- **FlakeRadar today:** job attribution and runner grouping catch job-level
  infrastructure failures. A JUnit report in which a database outage fails most
  tests, followed by a passing re-run on the same SHA, gives many tests a proven
  flake (score ≥ 0.6) at once.
- **Mapping:** at processing, compute the failing share of a Run; above a
  threshold, mark the Run `infra_suspect` and exclude it from scoring (keep the
  executions for the detail panel).
- **Open questions:** threshold and minimum test count (a 3-test suite at 100%
  failure is not an outage). Should excluded runs count toward fail streak?

### 5.10 PR comment and a shipped PR gate — M

- **Competitors:** Datadog and Trunk post GitHub PR comments; Datadog PR Gates
  block on "new flaky test".
- **FlakeRadar today:** the summary action writes to the job summary, which is
  less visible than a PR comment. `ci_run_failures` has the data for a gate.
- **Mapping:** the action posts or updates one sticky PR comment using the
  workflow's own `GITHUB_TOKEN`. This keeps ADR 0004's rule that the instance
  never calls the CI provider. One comment per CI run needs a final job that
  waits for all reports.
- **Open questions:** one comment per job or per run? Fork PRs cannot read the
  FlakeRadar secret.

### 5.11 Code owners and owner routing — M

- **Competitors:** Datadog and Trunk read CODEOWNERS; Datadog routes
  notifications by owner; Trunk puts owners in tickets and webhooks.
- **Mapping:** the report action uploads `.github/CODEOWNERS` (or resolved
  owners per file) with the report; processing matches the Test's file. Many
  runners give no file (`docs/ci-integration.md#test-locations`), so coverage
  is partial.
- **Open questions:** store owners per Test or resolve at read time? Route
  webhooks by owner, or only add `owners` to the payload?

### 5.12 Notifications: more events, formats and guarantees — S to M

- Broken and escalation events (S). Trunk sends every status change; ADR 0009
  sends nothing when a flaky test becomes broken. A `test.broken` event fits
  `fail_streak` crossing `BROKEN_STREAK`.
- Per-URL format (S). ADR 0009 notes one format per instance as a limitation.
- Microsoft Teams format (S).
- Weekly digest (S/M): Datadog (beta) and Trunk (on request). Score history
  already holds the data.
- Durable delivery (M): ADR 0009 says a `webhook_outbox` table "is the change to
  make first if somebody needs guaranteed delivery".
- Request signing: neither Datadog nor Trunk pages read describe it. Not a
  verified gap.

### 5.13 Jira / Linear ticketing — M each

- **Competitors:** Trunk built-in Jira, Linear, Asana with create, reopen within
  30 days, update on escalation, close on healthy. Datadog Jira via Work
  Management.
- **FlakeRadar today:** GitHub only. The issue link stores only a number in the
  test's own repo (ADR 0008).
- **Mapping:** a provider column on the link, and a filer per provider. ADR 0008
  already names a URL/provider column as "can come later as an additive
  migration".
- **Open question:** is there a real Jira or Linear user for this internal
  instance? The webhook path (5.12) may be enough.

### 5.14 Reopen instead of refile — S

- **Competitors:** Trunk reopens a ticket closed within a window (default 30
  days) when the test regresses.
- **FlakeRadar today:** a completed close clears the link, and the next flake
  files a new issue (`docs/github-issues.md#duplicate-issues`).
- **Mapping:** keep the closed number and close time; reopen when it is inside
  the window.

### 5.15 Impact metrics — S/M

- **Competitors:** Trunk "PRs impacted" (default sort) and "PRs rescued by
  quarantining"; Datadog wasted time and failed pipelines.
- **Mapping:** count distinct CI runs (and non-default branches) where a test
  failed in the window, from `test_runs`. Time lost = failing execution
  durations, which are already stored. Add `sort=impact`.
- **Open question:** does the score stay the default sort?

### 5.16 Duration analysis: slow tests and timeout inflation — S/M

- **Competitors:** Trunk slow-test and timeout-inflation monitors, longest-running
  table; Harness `test is slow` condition.
- **FlakeRadar today:** durations stored per execution, never analysed.
- **Mapping:** p50 and p95 per Test over the score window at rescore; a
  "timeout inflation" flag (failing p50 ≥ 2× passing p95) helps the `timing`
  category.
- **Open question:** is this in FlakeRadar's scope (flakiness), or a separate
  test-health concern?

### 5.17 Environment variants — M/L

- **Competitors:** Trunk `--variant`; Datadog configuration tags (OS, runtime)
  used in flakiness detection.
- **FlakeRadar today:** a Test is unique per (Repo, Project, suite, classname,
  name). A test flaky only on Windows mixes with its Linux runs. The workaround
  is one Project per variant.
- **Open question:** add a variant to Test identity (a migration and an ingest
  parameter), or document the project-per-variant pattern?

### 5.18 Labels and an event timeline — M

- **Competitors:** Trunk labels with provenance; Trunk Events tab and audit
  logs.
- **FlakeRadar today:** no labels; no record of quarantine changes, tier
  changes or issue links over time (score history is numbers only).
- **Constraint:** FlakeRadar has no user identity, and the quarantine toggle has
  no auth by design (internal deployment). An audit log could record "what and
  when", but not "who".

### 5.19 MCP additions — S each

- A read tool for the active settings (threshold, window, broken streak, filing
  gate), like Datadog's settings and policies tools.
- A setup tool or prompt that returns the CI snippet for a runner, like Trunk
  `setup-trunk-uploads` and Datadog onboarding.
- **Conflict:** a quarantine or state-change write tool (Datadog
  `update_datadog_flaky_test_states`) conflicts with "quarantine stays a human
  decision" and ADR 0008. Do not add.

### 5.20 Server-side AI investigation and fix PRs — conflicts with ADR 0006

- **Competitors:** Datadog AI categories and Bits fix PRs; Trunk investigations
  (quota-limited) plus a webhook that drives a Cursor Automation.
- **FlakeRadar stance:** ADR 0006 rejects per-failure LLM calls (cost,
  nondeterminism, outbound access). The MCP server already gives an external
  agent the location, traceback and history to do the same work. Trunk's own
  fix PR also comes from an external agent.
- **Low-cost option that keeps the ADR:** an MCP prompt that tells an agent how
  to investigate one test with the existing tools.

### 5.21 Other gaps, low priority

- Non-JUnit formats (Bazel BEP, XCResult): M each.
- Fork PR uploads: M; needs a public, scoped token, which fights the
  internal-only deployment.
- Aggregate charts (newly flaky, recovered, quarantined per day): S/M from score
  history.
- Test selection (Datadog TIA, Harness Test Intelligence): out of scope.

## 6. What FlakeRadar does that the competitors' docs do not show

- **CI job flakiness** with explained/unexplained attribution, summary-job
  handling and failures grouped by runner (`docs/adr/0004-pushed-ci-jobs-and-attribution.md`).
  No Trunk or Harness page read describes job-level flake scoring. Datadog's
  pipeline docs were not read, so this is unverified for Datadog.
- **Self-hosted and open source:** one binary and PostgreSQL, push-only, no
  outbound access needed except the optional GitHub calls.
- **One continuous score** that combines a default-branch flip rate, sample
  damping and a proven-flake floor. Trunk reaches a similar split by
  configuration (pass-on-retry on all branches, rate monitors on `main`), but
  as separate binary monitors.
- **Quarantine with an expiry** that maintenance enforces (7, 14 or 30 days).
  Harness only advises "7 days max"; Trunk's quarantine pages describe no expiry.
- **GitHub issue lifecycle:** gated auto-filing, links from other filers,
  `not_planned` treated as "known, accepted", and a "fix appears to hold"
  comment at a clean streak. Trunk handles GitHub Issues only through a webhook
  transformation with no lifecycle.
- **Deterministic failure categories** with no per-failure cost, filterable in
  the API and MCP.
- **MCP links issues** (`link_issue`) so an agent that files an issue prevents a
  duplicate from FlakeRadar's own filer.
- **Prometheus metrics** and a failed-report retry and dismiss queue for the
  operator.
- **New-flaky-in-N-days** separate from trend, in the leaderboard, API and MCP.
