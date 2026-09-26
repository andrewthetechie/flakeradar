# FlakeRadar

Self-hosted flaky-test and flaky-job detection: CI uploads JUnit reports and CI job results, and FlakeRadar scores how often each Test's or Job's outcome flips between pass and fail.

## Language

### Where tests live

**Repo**:
A source repository, identified by its lowercase `owner/name` (for example `andrewthetechie/writers-app`). Unique across the instance.
_Avoid_: Repository slug, project (for a repository)

**Project**:
A named test suite inside a Repo, such as `frontend`, `backend` or `e2e`. Unique within its Repo, and the same name can exist in other Repos. A Repo with a single suite uses the project `default`.
_Avoid_: Suite (that is a JUnit concept), component, package

**Test**:
One logical test case, identified by its suite, classname and name within one Repo and Project. It is the same Test across runs, branches and CI providers.
_Avoid_: Test case (in user-facing text), spec

**Project root**:
The directory of a Project inside its Repo, such as `frontend`. File paths that a Project reports are relative to it.

**Pipeline**:
A named CI workflow in a Repo, such as the GitHub Actions workflow `.github/workflows/ci.yml`. It groups Jobs and is not scored itself.
_Avoid_: Workflow (in provider-neutral text), build

**Job**:
One job within a Pipeline, identified by its name within its Pipeline. Each matrix leg is its own Job, such as `test (ubuntu, 3.12)`. A Job belongs to a Repo, not to a Project, because one Job can run several Projects' tests.
_Avoid_: Check, step, Run (that is a processed Report)

**Location**:
The file (relative to the Project root) and line where a Test is defined, as the latest report gave them. A Location is not part of a Test's identity, and many runners report none.

### What CI reports

**Report**:
One upload from CI, of one of two kinds. A JUnit report is one JUnit XML file for a Repo and Project, and it becomes a Run. A Pipeline report is the Job results of one attempt of one Pipeline run, and it becomes Job executions. A Report is pending until it is processed. Then it has either been processed, or it has failed and is kept with its error.
_Avoid_: Upload, payload

**Run**:
The processed result of one Report: the Executions for one Repo and Project at a specific commit SHA.
_Avoid_: Build, Job (that is a CI job)

**Execution**:
One try of a Test by the test runner, and its outcome (passed, failed, error or skipped), within one Run. When the test runner retries a Test inside the same Run, each retry is its own Execution, numbered from 0 in the order they ran. A CI re-run is not a retry: it produces a new Run.
_Avoid_: Result

**Job execution**:
One Job's outcome (passed, failed or skipped) in one attempt at a specific commit SHA. A re-run of the same CI run is a new attempt and a new Job execution. A cancelled Job counts as skipped.
_Avoid_: Job run, build

**Failure message**:
The short, one-line reason that a failing Execution gives, such as `AssertionError: expected 3, got 4`.

**Failure category**:
The likely cause of a failing Execution: timing, network, environment, assertion or other. Rules assign it from the Failure message and Failure details. A Test's Failure category is the most common one among its recent failing Executions.
_Avoid_: Root cause (the category is a guess, not a diagnosis)

**Failure details**:
The full output that a failing Execution gives: the traceback and any captured stdout or stderr.
_Avoid_: Stack trace (details can hold more than a trace)

### How flakiness is judged

**Flakiness score**:
A number from 0 to 1 that measures how often a Test's or Job's outcome flips between pass and fail, with recent flips weighted more. A Test that always fails scores 0: it is broken, not flaky. Flips count only on the Repo's Default branch. On other branches, only Proven flakes count, because a fail then a pass on a new commit there is usually a fix, not a flake.

**Default branch**:
The Repo's main line of development, such as `main`, as CI last reported it. Until it is known, every branch counts as the Default branch.

**Proven flake**:
A commit SHA on which the same Test or Job both passed and failed. This proves nondeterminism and puts a floor under the Flakiness score.
_Avoid_: Confirmed flake, same-SHA flip (in user-facing text)

**Explained failure**:
A failed Job execution in which at least one Test execution uploaded by that same Job attempt also failed. The failing Tests explain it, so it does not count against the Job's Flakiness score.

**Unexplained failure**:
A failed Job execution with no failing Test execution from the same Job attempt: a failure from setup, infrastructure or a step outside the test runner. Only these count against a Job's Flakiness score.

**Score history**:
A Test's Flakiness score at the end of each day, with that day's counts of Executions, failures and Proven flakes. It shows whether a Test is getting worse or better.

**Clean streak**:
The number of non-skipped Executions of a Test since its last failure. After a fix, a growing Clean streak shows that the fix held.

**Flake threshold**:
The Flakiness score at or above which a Test counts as a Flaky test.

**Flaky test**:
A Test whose Flakiness score is at or above the Flake threshold.

**Suspect test**:
A Test whose Flakiness score is above 0 but below the Flake threshold.

**Stable test**:
A Test whose Flakiness score is 0.

**Quarantine**:
A reversible human decision that marks a Test as one the test runner may skip. FlakeRadar never quarantines a Test on its own, and agents cannot quarantine through the MCP server.
