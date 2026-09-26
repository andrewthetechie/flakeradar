---
status: proposed
---

# Keep a daily score snapshot per Test, and a Clean streak

The UI and API show only the current Flakiness score, so nobody can see if a Test is getting worse or if a fix held. We now keep one **Score history** row per Test per day: the Flakiness score at the end of that day, plus that day's Executions, failures and Proven flakes. The processor writes it in the same transaction as the rescore, as an upsert on (Test, day), so later Runs on the same day replace the score and add to the counts. Rows are kept for 365 days, separately from Executions (90 days). The rescore also caches a **Clean streak** on the Test: the number of non-skipped Executions since its last failure. The detail panel shows a sparkline of the score and the Clean streak. The leaderboard and the MCP server show a trend (worsening, improving or steady) from the score today against the score 14 days ago. After a fix, the Clean streak grows and the score decays, which shows that the fix held.

## Considered Options

- Recompute the history from Executions at read time: rejected. Retention deletes Executions after 90 days, and replaying the scoring window for each point on each request is expensive.
- A snapshot for each Run: rejected. Busy Repos run hundreds of times a day, and daily points are enough for a trend line.
- A snapshot only for Flaky and Suspect Tests: rejected. A Stable Test that becomes flaky then has no baseline to compare against.
- A manual "mark as fixed" action: deferred. The Clean streak and the sparkline answer "did it hold?" without a new human workflow. A marker can come later if people ask for it.

## Consequences

- The snapshot stores the score as it was, so a change to the scoring rules (for example the Default branch rule in ADR 0004) shows as a step in the line. Old points are not rewritten.
- A Test that did not run on a day has no row for that day. The chart carries the last value forward.
- At about 5,000 Tests the table grows by up to 5,000 rows a day, about 1.8 million rows at 365 days. The table has no text columns, so this is acceptable, and the retention job prunes it.
