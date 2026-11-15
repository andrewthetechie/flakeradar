# Notifications

FlakeRadar can post a message when a test or a job becomes flaky, when it
recovers, and when a test becomes broken. This is for teams who work in Slack and
do not want an issue for each flaky test.

## Set up

Set `FLAKERADAR_WEBHOOK_URLS` to a comma-separated list of URLs, and choose a
format:

```bash
FLAKERADAR_WEBHOOK_URLS=https://hooks.slack.com/services/T000/B000/xxxx
FLAKERADAR_WEBHOOK_FORMAT=slack
FLAKERADAR_PUBLIC_URL=https://flakeradar.example.com
```

## Events

FlakeRadar sends a message when a test or job changes tier — when it becomes
flaky, or when its flaky state is cleared — and when a test becomes broken. A
test or job becomes flaky when its score reaches the flake threshold, and stays
flaky until its score falls below the recover threshold. A test becomes broken
when its fail streak on the default branch reaches the broken threshold, and a
pass on the default branch resets that streak. A job has no fail streak, so it
never sends a broken event:

| Event | When |
|---|---|
| `test.flaky` | The score of a test reached the flake threshold, setting its flaky state. |
| `test.recovered` | The score of a test fell below the recover threshold, clearing its flaky state. |
| `test.broken` | The fail streak of a test on the default branch reached the broken threshold, marking it broken. |
| `job.flaky` | The score of a job reached the flake threshold, setting its flaky state. |
| `job.recovered` | The score of a job fell below the recover threshold, clearing its flaky state. |

FlakeRadar sends nothing in these cases:

- The score moves, but the test or job stays in the state it had. A test that is
  flaky and whose score sits between the recover threshold and the flake threshold
  stays flaky, so those reports send nothing — that band is what stops a score
  that hovers at the flake threshold from announcing flaky and recovered on every
  report.
- A report changes the default branch. That change scores the whole repo
  again, so the report sends no events at all, also for the tests and jobs
  that it changed. The log records how many events FlakeRadar dropped.
- Retention deletes old executions, and a score falls as a result. The log
  records how many events FlakeRadar dropped.
- A test that is [broken](concepts.md#broken-tests) leaves the flaky tier. A
  broken test has not recovered, so that crossing sends no `test.recovered`. The
  break itself sent `test.broken`, when the fail streak first reached the broken
  threshold, and a test that stops being broken sends nothing: the pass that
  resets the streak is no event. So `test.flaky` or `test.broken` can be the last
  event that you see for a test, and you hear about it again when its flaky state
  moves — a score that reaches the flake threshold sends a new `test.flaky`, and a
  later score below the recover threshold sends `test.recovered`.
- A test or job that a person acknowledged. While the acknowledgement lasts,
  FlakeRadar sends nothing for it, whatever its score does: the team has said
  that flake is known and owned, so the channel needs no news about it. The
  state itself still moves — a report sets or clears the flaky state as usual
  — only the announcement is dropped, so a crossing the acknowledgement skipped
  is not sent when the acknowledgement ends. After it ends, the next crossing
  sends its event.

## Formats

`json` (the default) posts a list of event objects, one for each test or job
that the report changed. One report is one request:

```json
[
  {
    "event": "test.flaky",
    "repo": "acme/app",
    "project": "backend",
    "test": {
      "id": 42,
      "classname": "tests.test_auth",
      "name": "test_login",
      "flakiness_score": 0.42,
      "tier": "flaky",
      "confirmed_flake_count": 2,
      "failure_category": "timing"
    },
    "url": "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42"
  }
]
```

A job event has `"project": null` and a `job` object with `id`, `pipeline`,
`name`, `flakiness_score`, `tier` and `confirmed_flake_count`. Its `url` opens
the jobs view. FlakeRadar leaves out `url` when `FLAKERADAR_PUBLIC_URL` is
empty. A `test.broken` event has the same keys as the one above, with
`"event": "test.broken"` and the tier and score the test has now.

`slack` posts `{"text": ...}` with one line for each event:

```
FlakeRadar: acme/app / backend — test "test_login" became flaky (score 0.42) https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42
```

A broken test names the branch it broke on:

```
FlakeRadar: acme/app / backend — test "test_login" is broken on the default branch (score 0.00) https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42
```

## Delivery

Delivery is best effort:

- Each URL gets three attempts of five seconds each, with a short wait between
  them.
- FlakeRadar obeys the `retry-after` of a rate limit, up to ten seconds.
- FlakeRadar retries a timeout, a `5xx`, a `408` and a `429`. Any other `4xx`
  is a configuration error, and FlakeRadar does not retry it.

FlakeRadar keeps no queue on disk. A message is lost when FlakeRadar cannot
deliver it, or when it is still in the queue when the process stops. The queue
holds the messages of 1000 reports. When it is full, FlakeRadar drops the
oldest messages, so a long endpoint outage also loses messages.

The dashboard still shows the current tier, and the next report that changes
the tier sends a new message.
