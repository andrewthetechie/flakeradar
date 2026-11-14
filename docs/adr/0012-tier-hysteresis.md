---
status: proposed
---

# A flaky Test stays flaky until its score falls below a second, lower threshold

A Flakiness score that hovers near the Flake threshold crosses it on every other
Report, and every crossing was a Tier change: `test.flaky`, then `test.recovered`,
then `test.flaky` again, one message per flip of a Test whose flakiness had not
changed at all. A Test or Job therefore becomes flaky when its score reaches
`FLAKE_THRESHOLD` (shipped default `0.30`) and stays flaky until its score falls
below `RECOVER_THRESHOLD` (shipped default `0.20`). Between the two thresholds a
row keeps the state it already has, so the announcements and the tier follow the
Test's evidence instead of the last single Report.

## The state is stored, not derived

The state is the `flaky_since` timestamp on `test_cases` and on `jobs` (migration
`0003_flaky_since`): NULL means not flaky, and a set value is the instant the row
entered the tier. It is written by the rescore and by nothing else. `Update::of`
calls `next_flaky_since` (`crates/flakeradar-core/src/helpers.rs`) with the row's
stored value, its new score, both thresholds and the processor clock's instant;
`old_states` reads the stored value before the write so a rescore never compares a
row against itself; and the bulk `UPDATE` writes the result back with the score. A
row with no stored state counts as not flaky, so it enters the tier on
`score >= FLAKE_THRESHOLD`, and a stored state clears only on
`score < RECOVER_THRESHOLD` — clamped to the lower of the two, so a caller that
builds `ProcessingSettings` by hand cannot pass a recover threshold that makes the
state unholdable. `Settings::validate` refuses `FLAKERADAR_RECOVER_THRESHOLD` above
`FLAKERADAR_FLAKE_THRESHOLD` for the same reason.

The tier every Rust read answers is `tier_for(score, flaky_since.is_some())`, so
the tier in the API, the tier on a Notification's snapshot and the state the rescore
moved are one column read three ways, and they cannot disagree.

Storing the instant, rather than a replay, is the point. Score history is a daily
snapshot (ADR 0007) pruned by retention: it cannot say which side of a threshold a
row was on at the last Report, and answering that from it would re-decide the same
question on every read, with answers that drift as rows age out. One stored column
is written in the same transaction as the score it follows and is the same answer
for every surface.

Migration `0003_flaky_since` adds the nullable column to both tables and backfills
it: every row already at or above 0.30 gets `flaky_since = now()`. Without that,
each existing flaky row would start as not flaky and re-enter the tier on its next
Report, announcing a `test.flaky` for a Test that has been flaky for weeks.

## A Tier change is a move of the state

`crossed` (`crates/flakeradar-db/src/processing.rs`) compares the old and the new
`flaky_since`, not the score: NULL to set is `test.flaky` / `job.flaky`, set to
NULL is `test.recovered` / `job.recovered`, and a score that drifts inside the band
moves nothing and sends nothing. A Test that is now broken still sends no closing
event — the recovered rule is unchanged — and a Report that changes the Default
branch still sends nothing at all (ADR 0009).

## The filing gate is not a tier

The GitHub filing gate stays on the score. `github_issue_min_score`
(`FLAKERADAR_GITHUB_ISSUE_MIN_SCORE`) is a minimum over the cached Flakiness score,
not a tier: it asks how strong the evidence is, not which state the row holds, and
a score inside the band is still evidence worth filing. Moving it onto
`flaky_since` would stop filing for Tests whose score had dropped to 0.25 while
their issue is still open, which is the opposite of what the gate is for.

## The SQL tier filters are the known gap

The leaderboard's tier filter, the summary counts, the new-flaky count and the
`/metrics` gauges are SQL over the cached score (`reads.rs` and `metrics.rs`), and
they still compare `flakiness_score` with the Flake threshold. A list filtered by
score and a `tier` field read from `flaky_since` can therefore disagree for a row
inside the band; the parity replay reports exactly those goldens as the allowed
difference `tier-hysteresis` (`tests/parity/allowed-differences.json`). Moving each
SQL comparison to the column is the follow-up, and until it lands the dashboards
read the tier honestly but filter it approximately.

## Considered Options

- **Deriving the state from Score history**: rejected. The history is daily and
  pruned (ADR 0007), so it cannot date a crossing, and a replay per read is the
  cost ADR 0007 already rejected for a smaller question.
- **Hysteresis on the Notifications only, with the tier still from the score**:
  rejected. The tier in the API would keep blinking while the events stayed quiet,
  so the two surfaces would answer differently for the same Test.
- **A boolean plus a clean-streak counter instead of a second threshold**:
  rejected. The score already is a decayed flip rate; a second counter answers the
  same question in another unit, with its own threshold to tune.
- **Recover threshold equal to the Flake threshold**: that is today's rule, and it
  stays reachable — `FLAKERADAR_RECOVER_THRESHOLD=0.30` reproduces it. The shipped
  default narrows the band; it does not remove the choice.

## Consequences

- The tier is path-dependent: two rows with the same score can sit in different
  tiers, because one of them has crossed and not yet fallen far enough back. What
  a tier means is stated in `CONTEXT.md` and `docs/concepts.md`, not derived from
  the score.
- Recovery is stricter than entry: a row leaves the flaky tier only through a
  score below 0.20, so `test.recovered` fires later than it did. Setting the
  recover threshold equal to the flake threshold is the documented way back.
- Rows the backfill marked announce nothing on their first Report after the
  upgrade: they are already flaky, and the rescore sees stored state, not a
  crossing.
- `docs/notifications.md`, `docs/concepts.md`, `docs/configuration.md` and
  `.env.example` state the rule and both defaults, and
  `every_setting_defaults_to_the_documented_value` pins `0.20` to
  `defaults.rs`.
