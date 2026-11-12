# How FlakeRadar works

This page tells how FlakeRadar organizes data, how it scores tests and jobs,
and what the dashboard shows.

## Repos and projects

A repo is a repository (`owner/name`). A project is a test suite in a repo. For
example, `andrewthetechie/writers-app` can have the projects `frontend`,
`backend` and `e2e`. Another repo can have projects with the same names, and
FlakeRadar keeps them separate. A repo with one suite can omit `project`, and
FlakeRadar then uses `default`. If a test is flaky on only one OS, runtime or
matrix leg, one project per variant keeps its score separate from its clean
runs; see [One project per variant](ci-integration.md#one-project-per-variant).

FlakeRadar creates a repo or a project when it gets the first upload for it.
It removes a repo or a project only through the admin API. Thus a typo in
`repo=` or `project=`, demo data, or an archived repo does not have to stay in
the pickers. See [Delete a repo or a project](api.md#delete-a-repo-or-a-project).

In the dashboard, select a repo. Then you can select one of its projects. In
the "All repos" view, each row shows `repo · project`.

## How scoring works

For each test, FlakeRadar uses the last 50 executions. `FLAKERADAR_SCORE_WINDOW`
changes this number. The score has three parts:

1. Flip score. FlakeRadar calculates the rate of changes between pass and fail
   in consecutive executions. Recent changes have more weight. A test that
   alternates each time scores 1.0. A test that always fails or always passes
   scores 0.
2. Sample damping. One flip in two runs is a 100% flip rate, but it is weak
   evidence. FlakeRadar reduces the score until the test has 7 executions.
3. Same-SHA floor. A commit with both a pass and a fail is a proven flake. The
   score is then 0.6 or higher. Each additional proven flake adds 0.1, to a
   maximum of 1.0.

FlakeRadar ignores `skipped` executions. It counts `error` as a failure.

Tests are in three tiers:

- flaky: the score is at or above the threshold (`FLAKERADAR_FLAKE_THRESHOLD`,
  default `0.30`).
- suspect: the score is above 0 and below the threshold.
- stable: the score is 0.

The leaderboard does not show stable tests unless you ask for them. The same
rules and tiers apply to CI jobs. For a job, only unexplained failures count.
See [Explained and unexplained job failures](ci-integration.md#explained-and-unexplained-job-failures).

### The default-branch rule

This rule applies to tests and jobs. Flips count only on the default branch of
the repo. A test or job that fails on a PR branch and then gets a fix must not
look flaky. Thus the flip score uses only the newest executions on the default
branch.

Proven flakes count on all branches, because a pass and a fail on the same
commit show nondeterminism on the same code. Until a report gives the default
branch, all branches count.

## Broken tests

A test that fails on the default branch in `FLAKERADAR_BROKEN_STREAK`
consecutive runs (default 5) is broken, not flaky. It scores 0 like a stable
test. A broken test has been red on the default branch and has not fixed
itself. It is not a new failure that a branch brought in.

FlakeRadar marks a broken test with a `broken` flag. It does not change the
score or the tier. The dashboard shows a badge on the leaderboard row and in
the detail panel, and lists broken tests under the `?broken=1` filter.

FlakeRadar ignores `skipped` rows when it counts the streak. A pass sets the
streak to 0. Set `FLAKERADAR_BROKEN_STREAK=0` to turn broken detection off.

## Score history and trends

FlakeRadar keeps a daily score history. Each test and each job gets one row for
each UTC day. The row contains these values:

- the flakiness score and the proven flake count at the end of the day.
- the number of executions and failures on that day.

Each test and job also has these values:

- A trend (`worsening`, `improving` or `steady`). FlakeRadar compares the
  current score with the newest history row that is at least 14 days old.
- A clean streak. This is the number of non-skipped executions on the default
  branch since the last failure. An explained job failure does not stop the
  clean streak of a job.

The leaderboard and the API can also list what became flaky in the last N days,
a new flaky test or job. The trend arrow cannot show this, because the arrow
also changes when an old flaky test becomes worse.

The detail panel shows the score for the last 90 days as a sparkline.
FlakeRadar deletes history after 365 days (`SCORE_HISTORY_RETENTION_DAYS`).

When retention deletes the executions of a test or job
(`EXECUTION_RETENTION_DAYS`), FlakeRadar calculates its score again from the
executions that remain. With no executions left, the score becomes 0, and the
test or job leaves the default leaderboard.

## Failure categories

Each failing test execution gets a likely cause: `network`, `environment`,
`timing`, `assertion` or `other`. Fixed rules set the category. FlakeRadar
tries the rules in this order, from the most specific to the least specific:

1. network
2. environment
3. timing
4. assertion

A failure that matches no rule is `other`. FlakeRadar checks the failure
message first. It checks the failure details only when the message matches no
rule.

For example, `Timed out 5000ms waiting for expect(...)` is `timing`, and
`connect ECONNREFUSED` is `network`. The category of a test is the most common
category in its last `SCORE_WINDOW` executions.

The category is a likely cause, not a diagnosis. Read the failure details
before you decide on the cause. Failures that FlakeRadar stored before
categories existed have no category (`null`).

## Quarantine

In the dashboard, you can mark an unreliable test as quarantined. A small
dialog lets you record an optional reason and when to look at the test again:
Never, 7, 14 or 30 days.

Before a run, your test runner gets the quarantine list for its repo and
project:

```bash
curl -sS "$FLAKERADAR_URL/api/quarantine?repo=$GITHUB_REPOSITORY&project=backend" \
  -H "X-API-Key: $FLAKERADAR_TOKEN"
# -> [{"suite","classname","name","fingerprint","file","line","quarantined_at","reason","expires_at"}, ...]
```

The runner matches each item on `classname` and `name` to find the tests to
skip.

Only a person can quarantine a test, and a person can remove the quarantine.
Removing the quarantine also clears the reason and the expiry. After the
`expires_at` time of a quarantine, maintenance clears it, so an expired
quarantine never continues to skip a test. FlakeRadar never skips a test by
itself, and the MCP server cannot quarantine a test.

### Skip quarantined tests in your runner

Ready-made consumers in `samples/quarantine/` fetch the quarantine list and
skip each quarantined test. They read `FLAKERADAR_URL`, `FLAKERADAR_TOKEN`,
`FLAKERADAR_REPO` and `FLAKERADAR_PROJECT`. When FlakeRadar cannot be reached,
they skip nothing.

- [The contract every consumer follows](../samples/quarantine/README.md)
- pytest: [flakeradar_quarantine.py](../samples/quarantine/pytest/flakeradar_quarantine.py)
- cargo-nextest: [quarantine-filter.sh](../samples/quarantine/nextest/quarantine-filter.sh)
- Vitest and Jest: [flakeradar-quarantine.mjs](../samples/quarantine/vitest-jest/flakeradar-quarantine.mjs)
- Go: [quarantine-skip.sh](../samples/quarantine/go/quarantine-skip.sh)
