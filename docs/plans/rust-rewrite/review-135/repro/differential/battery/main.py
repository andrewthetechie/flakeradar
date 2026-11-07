"""Run one leg of the differential sweep and write the report.

    python main.py --profile default --phases seed,d2,d1,d3,d5,d4

Both servers must already be running on fresh databases (see `run.sh`), one per
database, with the same settings. `seed` always runs first in the `default`
profile; the `retention` profile has its own seed because the prune deletes what
the default profile keeps.
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import sys
from pathlib import Path

from harness import KNOWN, Battery

HERE = Path(__file__).resolve().parent
PHASES = {
    "seed": ("seed", "seed"),
    "d1": ("d1_mcp", "mcp_tools"),
    "d2": ("d2_reads", "reads"),
    "d3": ("d3_reports", "failed_reports"),
    "d3r": ("d3_retry", "retries"),
    "d4": ("d4_concurrency", "concurrency"),
    "d5": ("d5_ingest", "ingest"),
    "d6": ("d6_retention", "retention"),
    "d7": ("d7_epochs", "epochs"),
}


def run(phase: str, battery: Battery) -> None:
    module_name, function = PHASES[phase]
    module = importlib.import_module(module_name)
    getattr(module, function)(battery)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--py", default=os.environ.get("BATTERY_PY", "http://127.0.0.1:18081"))
    parser.add_argument("--rs", default=os.environ.get("BATTERY_RS", "http://127.0.0.1:18082"))
    parser.add_argument("--pg", default=os.environ.get("BATTERY_PG", "t23-pg"))
    parser.add_argument("--phases", default="seed,d2")
    parser.add_argument("--profile", default="default")
    parser.add_argument("--out", default=str(HERE / "out"))
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    battery = Battery(py=args.py, rs=args.rs, pg_container=args.pg)
    battery.idle()

    summaries = []
    for phase in [p for p in args.phases.split(",") if p]:
        before = len(battery.differences)
        run(phase, battery)
        summary = battery.summary()
        summary["newDifferencesInPhase"] = len(battery.differences) - before
        summaries.append(summary)
        print(json.dumps(summary), flush=True)
        # Written after every phase, so a phase that fails still leaves the
        # differences the earlier ones found.
        _write(out, args.profile, summaries, battery)

    lines = [
        f"# Differential sweep — {args.profile} profile",
        "",
        f"{sum(s['cases'] for s in summaries)} comparisons: "
        f"status, compared headers and body per request, and row-level checks.",
        "",
        "| Phase | Comparisons | New differences | Known differences |",
        "|---|---|---|---|",
    ]
    for summary in summaries:
        known = ", ".join(f"{key} x{count}" for key, count in summary["knownDifferences"].items()) or "—"
        lines.append(
            f"| {summary['phase']} | {summary['cases']} | {summary['newDifferencesInPhase']} | {known} |"
        )
    lines += ["", "## New differences", ""]
    if battery.differences:
        lines += [d.render() for d in battery.differences]
    else:
        lines.append(
            "None. Every difference the sweep found is one `differences.md` records; "
            "the rows above name them by their ledger id."
        )
    lines += ["", "## Known differences this sweep exercised", ""]
    for key, count in sorted(battery.known_counts.items()):
        lines.append(f"- `{key}` x{count} — {KNOWN[key]}")
    if battery.notes:
        lines += ["", "## Notes", ""] + [f"- {note}" for note in battery.notes]
    (out / f"report-{args.profile}.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:12]))
    return 1 if battery.differences else 0


def _write(out: Path, profile: str, summaries: list[dict], battery: Battery) -> None:
    """The machine-readable report, after each phase and at the end."""
    report = {
        "profile": profile,
        "phases": summaries,
        "differences": [d.__dict__ for d in battery.differences],
        "notes": battery.notes,
    }
    (out / f"report-{profile}.json").write_text(json.dumps(report, indent=2, default=str) + "\n")


if __name__ == "__main__":
    sys.exit(main())
