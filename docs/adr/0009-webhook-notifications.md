---
status: proposed
---

# A Notification is rendered from a snapshot and is lossy by design

Teams that work in Slack want to hear when a Test or Job becomes flaky, without
an issue for every flake. FlakeRadar now posts a **Notification** to every
configured Webhook URL when a Test's or Job's tier crosses the Flake threshold,
in one direction or the other (`test.flaky`, `test.recovered`, `job.flaky`,
`job.recovered`).

Detection lives inside `rescore` and `rescore_jobs`, which *return* the
crossings and never emit them. Two other callers — `rescore_repo`, which runs
when a Report changes the Repo's Default branch, and the retention prune — must
stay silent: a Default branch change rescores the whole Repo at once, so
emitting there would send hundreds of messages for one Report. A caller that
must stay silent ignores the return value; the two processing paths put it on
`ProcessOutcome.tier_changes`.

Each crossing carries a snapshot of the values that produced it: repo, project,
classname, name, the new score and tier, the Proven flake count, and the Failure
category. Rendering reads nothing, so a message can never contradict the
direction of its own event, and the delivery task needs no database session.

Delivery is best effort: a bounded in-process queue, three attempts per endpoint,
and no record kept of what was sent.

## Considered Options

- **A `webhook_outbox` table**, written in the processor's transaction and
  drained by the leader: at-least-once, and it survives a crash. Rejected for
  now because it costs a table, a migration and a drain loop, and a lost
  "became flaky" is survivable — the dashboard still shows the tier, and the
  next tier change sends a new message. This is the change to make first if
  somebody needs guaranteed delivery.
- **Hydrating the payload from the database at send time**, which is what the
  GitHub task does. Rejected: a message delivered a few seconds later could show
  a score below the threshold while claiming the Test just became flaky.
- **One request per event**, and an object payload when there is only one.
  Rejected: grouping by Report is one request, and a list for every request
  keeps the consumer's shape constant. Slack gets one `text` with one line per
  event, because Slack has no batch form.
- **Storing the last-notified tier** so a restart can re-send. Rejected: the
  outbox problem in a smaller, partial form, with its own staleness rules.

## Consequences

- `FLAKERADAR_WEBHOOK_FORMAT` applies to every URL, so one instance cannot feed
  a Slack hook and a JSON consumer at the same time. Two instances, or two
  URLs of the same format, are the workaround.
- The task is leader-only and started/stopped with the advisory lock, like the
  GitHub task. Its queue holds 1000 batches and drops the oldest when full, so
  a long endpoint outage loses news instead of buffering without bound.
- A Webhook URL is a credential (a Slack hook URL *is* the token). Logs name an
  endpoint's host only, never the URL.
- `FLAKERADAR_PUBLIC_URL` is introduced here and shared with the GitHub
  integration (#33); whichever lands second rebases onto the other.
