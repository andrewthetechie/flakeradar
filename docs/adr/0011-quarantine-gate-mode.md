---
status: proposed
---

# Quarantine is one boolean, and the gate is the workflow's choice

A **Quarantine** used to mean one thing: a Test the runner may skip, taken from
`GET /api/quarantine` before the run. Skipping has a cost. The Test stops producing
Reports, so the team loses the evidence that would say whether it is still flaky,
whether it has been fixed, or whether it has grown from the odd flake into a
**Broken test**. The summary action now offers the other mode: with `gate: true`
the workflow runs every Test, quarantine included, and the action fails the job
only for a failure that is not quarantined. A quarantined Test keeps producing
evidence and the job stays green.

Which mode a quarantined Test gets is therefore each workflow's choice, made by one
input, and not a property of the Test. What FlakeRadar stores stays one boolean per
Test, with its optional reason and expiry, and `GET /api/quarantine` is unchanged:
a workflow that skips and a workflow that gates read the same flag and cannot
disagree about it.

The gate decides on this job's failures only, with the same `ci_job_id` filter as
the summary table, so a job never goes red over another job's flakes. (With
`ci_job_id` empty the filter keeps every failure of the attempt, so a gate-mode
caller passes its job id.) Inside those failures it fails the job on any failure
that is not quarantined, whatever its Tier: a new failure, a **Broken test**, or a
flaky Test a person has already acknowledged with an Issue link. Only the
quarantine flag clears a failure, because it is the one that says a person accepted
this Test as flaky. A failure whose newest `status` is `passed` passed on retry
(ADR 0005), so it is not a failure of this job and never trips the gate. When the
gate does fail the job, the step prints one `::error::` line naming the Tests
behind it.

When FlakeRadar cannot answer, the gate fails closed on the test step's outcome. An
instance that cannot be reached, a `summary-timeout` that expires with reports still
pending, a `truncated` failure list, or a Report that `failed` to process all leave
the action unable to say whether every failure of this job is quarantined. It then
defers to `test-outcome` — the outcome of the test step, passed by the caller — and
exits 0 only when that is `success`; any other value, including the empty default,
exits 1, with one `::notice::` line naming the rule that could not decide. The
summary table and the timeout and unreachable lines are written either way, before
the exit; with no summary target at all the gate still polls and decides, while the
gate-off case keeps its early exit. With `gate` false or unset, nothing here
changes: the action only reads and every path still exits 0.

## Considered Options

- **A per-test mode stored in FlakeRadar**, where the quarantine carries
  `skip` or `gate`: rejected. It costs a column and a migration of the existing
  boolean, and every consumer of the flag has to learn which of two things it now
  means. The two modes are not even exclusive: one repo may want to gate in CI and
  skip in a local runner. The workflow knows where it is running, so that is the
  place to choose.
- **Failing open when FlakeRadar cannot answer**: rejected. That is silently green
  for exactly the team whose instance is down, and the action cannot tell "all of
  this job's failures are quarantined" from "no answer". The test step does know
  whether its tests failed, so the decision goes there.
- **A verdict endpoint on the server**, which applies the rule and answers pass or
  fail: rejected. The failures endpoint already returns everything the rule needs
  (`quarantined`, `status`, `ci_job_id`), and a second place that knows the rule is
  a second place to keep in step with it.

## Consequences

- The summary action can now end a job red. A workflow that turns the gate on runs
  its test step with an `id` and `continue-on-error: true`, so the summary step
  rather than the test command decides the job, and passes that step's outcome as
  `test-outcome`. `docs/ci-integration.md` shows the two-step form.
- The action's description says it fails the job only in gate mode, which is what
  the script does.
- The skip list and `GET /api/quarantine` are unchanged, so every existing consumer
  keeps working. A workflow that gates simply does not consult the skip list.
- Quarantine stays a human decision: the gate never quarantines anything, and
  FlakeRadar still quarantines no Test on its own.
- A quarantined Test in gate mode keeps producing evidence — its Report lands like
  any other, so its score, Tier and Issue link stay live. That is the point of the
  mode, and it is why the gate is opt-in: `gate` defaults to `false`.
- A long attempt is an incomplete answer, not a clean one: more than 500 failures
  sets `truncated`, and the gate then falls back to `test-outcome` instead of
  guessing from the part it can see.
