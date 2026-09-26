# Track CI Jobs from pushed Pipeline reports, and count only unexplained failures

Some flakiness never shows up in JUnit: a runner loses its network, a setup step times out, or a whole job dies. We now track each CI **Job** (one matrix leg of one Pipeline) and score it with the same flip and Proven flake rules as a Test. CI pushes job results to FlakeRadar as a **Pipeline report**. FlakeRadar never calls the CI provider. On GitHub, a reporter workflow triggered by `workflow_run` lists the jobs of the attempt that just finished and pushes them in one batch. The payload does not depend on any one provider, so GitLab or Forgejo only need their own small reporter. Each re-run attempt produces new Job executions on the same SHA, which gives Proven flakes directly.

A JUnit upload can name the Job execution that produced it (on GitHub, `job.check_run_id`). When a Job execution fails and a Test execution from the same attempt also fails, the failure is **explained**. It is ignored when the Job is scored, like a skipped execution. Only **unexplained** failures count against a Job. The Job leaderboard then shows failures that are not caused by tests, and a flaky Test does not also make its Job look flaky. Attribution is recomputed each time the Job is rescored, because job results and JUnit reports can arrive in either order.

For both Tests and Jobs, flips count only on the Repo's **Default branch**. On other branches, only Proven flakes count. On a PR branch, "fail, push a fix, pass" is a fix, not a flake. This happens far more often with Jobs (lint and type-check jobs fail on every bad push), but it already made broken-then-fixed Tests look flaky. The Default branch comes from CI (in the Pipeline report, and optionally on JUnit ingest). Until it is known, every branch counts, which is the old behaviour.

## Considered Options

- Poll the CI provider's API from FlakeRadar: rejected. The instance does not need to reach GitHub, and a push API works for any provider.
- GitHub webhooks: rejected for the same reason. The instance is internal only.
- A summary job using `needs`: rejected, because `needs.<job>.result` merges all matrix legs into one result.
- Score Workflows as well as Jobs: rejected. A Workflow's result is only the combination of its Jobs' results, so scoring both counts each failure twice.
- One combined Test-and-Job score: rejected, because it mixes two different questions into one number.

## Consequences

- Existing Test scores change once a Repo's Default branch is known. All of that Repo's Tests are rescored at that point.
- Job attribution needs the Job execution id on JUnit uploads. Uploads without it still work, but their failures can't explain a Job failure.
- Jobs have no Quarantine. A runner can't skip a job, and `continue-on-error` is a change to the workflow, not a FlakeRadar decision.
