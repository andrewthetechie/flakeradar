---
status: proposed
---

# Classify failures by rules at ingest, into five Failure categories

Flaky tests are easier to fix in batches when we know the likely cause. Each failing **Execution** now gets a **Failure category**: `timing`, `network`, `environment`, `assertion` or `other`. A pure function assigns it at ingest from the Failure message and Failure details, using an ordered list of regular expressions. The Failure message is checked first, and the Failure details only when the message matches nothing. The first match wins, and the categories are tried from most to least specific: `network`, `environment`, `timing`, `assertion`. The order matters: Playwright's `Timed out 5000ms waiting for expect(locator).toBeVisible()` contains an assertion but is a timing failure, and `connect ETIMEDOUT` is a network failure, not a timing one. Examples: `timing` matches `timeout`, `timed out`, `TimeoutError`, `deadline exceeded`; `network` matches `ECONNREFUSED`, `ECONNRESET`, `ENOTFOUND`, `socket hang up`, `net::ERR_`, HTTP 502/503/504, `ConnectionError`; `environment` matches `ENOSPC`, `MemoryError`, out-of-memory kills, `Permission denied`, `Target closed`, missing modules or files; `assertion` matches `AssertionError`, `expect(`, `assert`. A Test's Failure category is the most common one among the failing Executions in its score window, and the newest wins a tie. It is cached on the Test at rescore, like the Flakiness score. The leaderboard, the API and the MCP server can filter and group by it.

## Considered Options

- Ask an LLM to classify each failure: rejected. It costs money per failure, gives different answers for the same input, and needs outbound access that an internal instance may not have. An agent that needs a deeper diagnosis can read the Failure details through the MCP server.
- Classify at read time: rejected. Rescoring would have to load up to 16 KB of Failure details for each Execution in the window.
- User-configured rules from day one: deferred. The built-in list must prove itself first. Extra rules from an environment variable are easy to add later.
- A category from timing data alone (duration near a timeout): rejected. JUnit does not report the timeout, and runners write the timeout into the message anyway.

## Consequences

- The category is stored on each Execution, so a change to the rules applies only to new uploads. There is no backfill: Executions stored before this change have no category and age out with retention. A reclassify command can come later if the rules change often.
- The rules are a heuristic. The UI calls the value "likely cause", and a Test that is misclassified has no manual override in this first version.
- The categories depend on Failure messages. Runners that write poor messages (for example, only `FAILURE`) produce mostly `other`.
