# Track CI Jobs from pushed Pipeline reports, and count only unexplained failures

Some flakiness never shows up in JUnit: a runner loses its network, a setup step times out, or a whole job dies. We now track each CI **Job** (one matrix leg of one Pipeline) and score it with the same flip and Proven flake rules as a Test. CI pushes job results to FlakeRadar as a **Pipeline report**. FlakeRadar never calls the CI provider. On GitHub, a reporter workflow triggered by `workflow_run` lists the jobs of the attempt that just finished and pushes them in one batch. The payload does not depend on any one provider, so GitLab or Forgejo only need their own small reporter. Each re-run attempt produces new Job executions on the same SHA, which gives Proven flakes directly.

A JUnit upload can name the Job execution that produced it (on GitHub, `job.check_run_id`). When a Job execution fails and a Test execution from the same attempt also fails, the failure is **explained**. It is ignored when the Job is scored, like a skipped execution. Only **unexplained** failures count against a Job. The Job leaderboard then shows failures that are not caused by tests, and a flaky Test does not also make its Job look flaky. Attribution is recomputed each time the Job is rescored, because job results and JUnit reports can arrive in either order.

A Job can also be a **summary** Job: it runs no tests and only reports whether the Jobs it `needs` succeeded (the [alls-green](https://github.com/marketplace/actions/alls-green#why) pattern, used for branch protection). Such a Job fails whenever any Job it needs fails, so its own failure carries no information. The reporter marks it with `summary: true` in the Pipeline report, because no CI API exposes a Job's `needs` list. A failed summary Job is **explained** when another failed **non-summary** Job exists in the same CI run attempt, and an explained failure is never scored. A summary Job that fails with no failed non-summary Job (its own step broke) stays **unexplained**. Requiring a non-summary sibling keeps a chain of summary Jobs honest: if an inner Summary Job breaks on its own, the outer Summary Job is not explained by it.

For both Tests and Jobs, flips count only on the Repo's **Default branch**. On other branches, only Proven flakes count. On a PR branch, "fail, push a fix, pass" is a fix, not a flake. This happens far more often with Jobs (lint and type-check jobs fail on every bad push), but it already made broken-then-fixed Tests look flaky. The Default branch comes from CI (in the Pipeline report, and optionally on JUnit ingest). Until it is known, every branch counts, which is the old behaviour.

## Considered Options

- Poll the CI provider's API from FlakeRadar: rejected. The instance does not need to reach GitHub, and a push API works for any provider.
- GitHub webhooks: rejected for the same reason. The instance is internal only.
- A summary job using `needs`: rejected, because `needs.<job>.result` merges all matrix legs into one result.
- Infer a summary Job from a failure with no linked Run: rejected. It would hide real setup failures, for example a Job whose tests crashed before writing JUnit, by blaming an unrelated flaky test in the same run.
- Mark summary Jobs in the sample reporter's exclude list only: rejected as the fix. Each Repo would have to know the list, and FlakeRadar could still not tell which Job a failure belongs to.
- Score Workflows as well as Jobs: rejected. A Workflow's result is only the combination of its Jobs' results, so scoring both counts each failure twice.
- One combined Test-and-Job score: rejected, because it mixes two different questions into one number.

## Consequences

- Existing Test scores change once a Repo's Default branch is known. All of that Repo's Tests are rescored at that point.
- Job attribution needs the Job execution id on JUnit uploads. Uploads without it still work, but their failures can't explain a Job failure.
- The reporter must mark its summary Jobs (`summary: true`), because no CI API exposes a Job's `needs`. Until it does, a summary Job's failures are unexplained and count against it.
- A `summary` flag flip is picked up from the newest Pipeline report; it is not sticky.
- Jobs have no Quarantine. A runner can't skip a job, and `continue-on-error` is a change to the workflow, not a FlakeRadar decision.
