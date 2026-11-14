---
status: proposed
---

# A mass failure leaves its Run out of scoring, and deletes nothing

When most of a large Run fails together, the cause is usually one shared thing —
the runner, the network, a service the suite depends on — rather than one cause per
Test. Scored as ordinary evidence, such a Run makes a hundred Tests look flaky or
broken at once, and the team gets a hundred lines about one bad runner. A processed
Run is therefore stored as suspected infrastructure when both limits hold:

- it holds at least `INFRA_MIN_TESTS` distinct Tests (shipped default `20`), and
- at least `INFRA_FAIL_SHARE` of those Tests (shipped default `0.8`) failed or
  errored on their **last attempt**.

The last attempt per Test decides, so a Test that recovered on retry counts as a pass
and never pushes a healthy Run over the share. The floor is checked first, so a small
Run is never judged: a 3-Test Report that fails entirely has a broken Test, not a bad
runner, and a floor of 1 would call it infrastructure. Both numbers are settings —
`FLAKERADAR_INFRA_FAIL_SHARE` and `FLAKERADAR_INFRA_MIN_TESTS` — validated at
start-up like every other, and the processor writes
`test_runs.infra_suspect = true` (migration `0002_infra_suspect`) before the rescore
that reads the flag.

This is an exclusion, not a deletion. Nothing is removed or rewritten: the Run's
Executions stay stored, stay in the Report's counts and today's Score history counts,
and are listed by every read, with their Failure messages and details, so a person
can still see what the outage did. What drops them is the Test scoring window.
`test_window_ids_sql` leaves the Run's Executions out in both arms, inside the
`LATERAL` and before its `LIMIT`, so the Run neither scores nor consumes a window slot
that would push real history out. The flip score, the Proven flake count, the Clean
streak, the Fail streak and the dominant Failure category all read that window, so all
five ignore the Run. The surfaces that ask a different question keep seeing the
Executions: the filing gate reads its own overall window
(`test_window_ids_overall_sql`), and Job attribution is unchanged, so a failing Test
in a suspected Run still explains its Job's failure.

Each Run is judged alone. FlakeRadar never adds up a CI run across its Runs: a Repo
that splits its suite into Projects uploads several Runs per CI run, and a runner that
dies on one leg leaves the sibling Runs of the same CI run intact and scored. Judging
the sum would let one dead leg hide a real regression that another leg reported.

## Considered Options

- **Deleting the Run's Executions, or the Run**: rejected. It destroys the only
  evidence of the outage — the messages, the details, the fact that the Run happened
  at all — and every read that lists Executions would answer differently for no
  scoring gain. Retention already deletes old Executions on its own schedule.
- **Discounting the Run's weight in the score instead of excluding it**: rejected. A
  weight still lets a mass failure move every Test it touched, and there is no honest
  weight for "we do not know why this failed". The window is all-or-nothing per
  Execution, so exclusion is the shape the code already has.
- **Deciding from the Failure category instead of the share**: rejected. The category
  is a guess from the Failure message (ADR 0006), and a runner outage often reports a
  plain assertion. The share needs no message at all.
- **Judging the CI run across its Runs**: rejected. The Runs of one CI run are separate
  uploads that can arrive in any order, and a Run that never arrived is not a signal;
  see the paragraph above.

## Consequences

- A Run is judged only as it is processed. Rows stored before this change keep
  `infra_suspect = false`, and nothing re-marks them: the marking reads the last
  attempts the Report gave, which the processor has and a later run would have to
  rebuild.
- A suspected Run contributes nothing to a Test's score, so a Test whose only new
  evidence is that Run keeps the score it had. Its Tier therefore does not change, and
  the Run fires no Tier-change Notification of its own. The filing gate is the one
  surface that can still act on the Run, because its failure counts read the overall
  window and a mass failure raises them.
- The flag is stored and not displayed: no API field or dashboard badge shows it yet,
  and no read reports why a Run is missing from a score. The follow-up adds both, so
  that the answer to "where did this go" is visible and not folklore.
- A genuine outage that fails most of a large suite is excluded rather than reported.
  That is the accepted trade: one bad runner is worth less evidence than a hundred
  false flakes, and the Executions are still there to read.
- `docs/concepts.md` and `docs/configuration.md` state the rule and both defaults, and
  `every_setting_defaults_to_the_documented_value` pins the numbers to
  `defaults.rs`.
