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
crossings and never emit them. The two processing paths put them on
`ProcessOutcome.tier_changes`. Two other causes must stay silent: a Report that
changes the Repo's Default branch, because it rescores the whole Repo at once
and emitting would send hundreds of messages for one Report, and the retention
prune, because losing old Executions is not news. Each logs how many Tier
changes it dropped and passes none on. A Report that changes the Default branch
also drops the Tier changes of the Tests or Jobs it touched itself, because
those were rescored under the new branch too.

A Test that leaves the flaky tier while it is broken sends nothing, because it
has not recovered. Its fix has no event of its own: the first pass is a new flip
that may cross the threshold and send another `test.flaky`, or may cross nothing
and send nothing. So a subscriber can see `test.flaky` with no closing event. We
accept that gap over a "recovered" message about a Test that fails every run.
Stating it is cheaper than a new event type or stored state.

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
