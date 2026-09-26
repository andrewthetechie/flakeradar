# 08 — Sparkline, trend marks and Clean streak in the UI

## Tracer-Bullet Outcome
The Tests and Jobs leaderboards show a trend mark next to each score: ↗ worsening, ↘ improving, → steady, or nothing when there is not enough history. The Test drawer and the Job drawer show the **Clean streak** and a sparkline of the last 90 days of Flakiness score. After a fix, a maintainer sees the line fall and the streak grow.

## User Story
As a maintainer, I want to see at a glance which flaky tests and jobs are getting worse, and whether a fix held, without reading raw numbers.

## Description
Frontend only. Add types to `api.ts`, two new small components (`Sparkline.tsx`, `TrendMark.tsx`), wire them into both leaderboards and both drawers, and update the fixtures. Hand-written SVG only, with no chart library.

## Context Pack
- Source decisions: `00-shared-context.md`, "Score history, Clean streak, trend". ADR 0007 ("The detail panel shows a sparkline of the score and the Clean streak. The leaderboard … show[s] a trend").
- Repo facts (API after tasks 06–07):
  - Every Test and Job object has `clean_streak: number` and `trend: "worsening" | "improving" | "steady" | null`.
  - `GET /api/tests/{id}/history` and `GET /api/jobs/{id}/history` have `score_history: [{ day: "2026-09-26", flakiness_score: 0.42, confirmed_flake_count: 1, executions: 3, failures: 1 }, …]`, oldest first, the last 90 days. Days with no activity have **no row**: carry the last value forward.
  - `frontend/src/format.ts` exports `formatScore(score: number): string` (two decimals, never rounds up).
  - Leaderboard score cell (`components/Leaderboard.tsx`, and the same shape in `components/JobLeaderboard.tsx`, where the variable is `j`):
    ```tsx
    <span className="w-9 text-right tabular-nums text-text-2">
      {formatScore(t.flakiness_score)}
    </span>
    ```
  - `components/TestDetail.tsx` starts its output with a stat row `<div className="mb-4 flex flex-wrap gap-x-8 gap-y-2">` holding blocks like:
    ```tsx
    <div>
      <div className="text-xs text-muted">Proven flakes</div>
      <div className="text-lg font-semibold tabular-nums">…</div>
    </div>
    ```
  - `components/JobDrawer.tsx`, after the title and issue line, renders:
    ```tsx
    <div className="mb-4 rounded-lg border border-line bg-surface-2 px-3 py-2 text-xs text-text-2">
      {history.unexplained_failures} unexplained failure
      {history.unexplained_failures === 1 ? "" : "s"} · {history.explained_failures}{" "}
      explained by tests (last {history.executions.length} executions)
    </div>
    ```
  - Tailwind colour classes that exist: `text-critical`, `text-good`, `text-muted`, `text-text-2`. The CSS variable `--seq-400` is the mid blue that the score bars use.
  - Fixtures to update: every `TestCase` (`App.test.tsx`, `components/Leaderboard.test.tsx`, `components/TestDrawer.test.tsx`), every `Job` (`App.test.tsx`, `components/JobLeaderboard.test.tsx`, `components/JobDrawer.test.tsx`), and every `History` / `JobHistory` (`components/TestDrawer.test.tsx`, `components/JobDrawer.test.tsx`, `App.test.tsx`).
- Non-goals: backend changes; axis labels or tooltips on the sparkline; a trend sort or filter; charting Proven flakes or counts.

## Delivery Strategy
- Shape: Normal tracer bullet
- Valid-state scope: `feat/flake-insights` after this draft

## Implementation Contract
- Expected files: new `frontend/src/components/Sparkline.tsx`, `frontend/src/components/Sparkline.test.tsx`, `frontend/src/components/TrendMark.tsx`. Edits: `frontend/src/api.ts`, `frontend/src/components/Leaderboard.tsx`, `frontend/src/components/JobLeaderboard.tsx`, `frontend/src/components/TestDetail.tsx`, `frontend/src/components/JobDrawer.tsx`, and the fixture files listed above.
- Interfaces and names:
  - `api.ts`:
    ```ts
    export type Trend = "worsening" | "improving" | "steady";
    export interface ScorePoint {
      day: string; // "YYYY-MM-DD", UTC
      flakiness_score: number;
      confirmed_flake_count: number;
      executions: number;
      failures: number;
    }
    // TestCase and Job gain:   clean_streak: number;  trend: Trend | null;
    // History and JobHistory gain:   score_history: ScorePoint[];
    ```
  - `components/Sparkline.tsx`:
    ```tsx
    import type { ScorePoint } from "../api";
    import { formatScore } from "../format";

    const PAD = 2;
    const DAY_MS = 86_400_000;

    /** SVG path for a step line of daily scores. x is placed by calendar day (gaps carry
     *  the last value forward); y maps score 0..1 to bottom..top. Coordinates are rounded
     *  to 2 decimals. */
    export function sparklinePath(points: ScorePoint[], width: number, height: number): string {
      const t0 = Date.parse(`${points[0].day}T00:00:00Z`);
      const idx = points.map((p) => (Date.parse(`${p.day}T00:00:00Z`) - t0) / DAY_MS);
      const span = Math.max(idx[idx.length - 1], 1);
      const r = (n: number) => Math.round(n * 100) / 100;
      const x = (i: number) => r(PAD + (i / span) * (width - 2 * PAD));
      const y = (s: number) => r(PAD + (1 - s) * (height - 2 * PAD));
      let d = `M${x(0)},${y(points[0].flakiness_score)}`;
      for (let k = 1; k < points.length; k++) d += ` H${x(idx[k])} V${y(points[k].flakiness_score)}`;
      if (points.length === 1) d += ` H${x(span)}`;
      return d;
    }

    export function Sparkline({ points, label, width = 240, height = 40 }: {
      points: ScorePoint[]; label: string; width?: number; height?: number;
    }) {
      if (points.length === 0) return <div className="text-xs text-muted">No score history yet.</div>;
      const first = points[0].flakiness_score;
      const last = points[points.length - 1].flakiness_score;
      return (
        <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img"
             aria-label={`${label}, ${points.length} days recorded: ${formatScore(first)} to ${formatScore(last)}`}
             style={{ display: "block", maxWidth: "100%" }}>
          <path d={sparklinePath(points, width, height)} fill="none" stroke="var(--seq-400)" strokeWidth={1.5} />
        </svg>
      );
    }
    ```
  - `components/TrendMark.tsx`:
    ```tsx
    import type { Trend } from "../api";

    const TREND: Record<Trend, { glyph: string; cls: string; title: string }> = {
      worsening: { glyph: "↗", cls: "text-critical", title: "Worsening: score up by 0.05 or more versus 14 days ago" },
      improving: { glyph: "↘", cls: "text-good", title: "Improving: score down by 0.05 or more versus 14 days ago" },
      steady: { glyph: "→", cls: "text-muted", title: "Steady versus 14 days ago" },
    };

    export function TrendMark({ trend }: { trend: Trend | null }) {
      if (!trend) return null;
      const t = TREND[trend];
      return <span className={`ml-1 text-xs ${t.cls}`} title={t.title} aria-label={t.title}>{t.glyph}</span>;
    }
    ```
  - `Leaderboard.tsx` / `JobLeaderboard.tsx`: right after the score `<span className="w-9 …">…</span>`, add `<TrendMark trend={t.trend} />` (`j.trend` in the Jobs leaderboard).
  - `TestDetail.tsx`: add a stat block `Clean streak` with value `{history.test.clean_streak}` (same markup as `Proven flakes`). After the stat row, add:
    ```tsx
    <div className="mb-4">
      <div className="mb-1 text-xs text-muted">Flakiness score, last 90 days</div>
      <Sparkline points={history.score_history} label="Flakiness score" />
    </div>
    ```
  - `JobDrawer.tsx`: in the summary box, append `· clean streak {job.clean_streak}` after the existing sentence (`job` is `history.job`, already in scope as `job`). Below that box, add the same sparkline block as in `TestDetail` (with `points={history.score_history}`).
- Verified external contracts: None.
- Behavior rules: the sparkline y-scale is fixed at 0..1, so lines are comparable across Tests. With one point, it draws a flat line across the full width. With no points, it shows the text `No score history yet.`
- Error and security rules: None.

## Acceptance Criteria
- [ ] `sparklinePath([{day:"2026-09-01", flakiness_score:0, …}, {day:"2026-09-03", flakiness_score:1, …}], 104, 14) === "M2,12 H102 V2"`.
- [ ] `sparklinePath([{day:"2026-09-01", flakiness_score:0.5, …}], 104, 14) === "M2,7 H102"`.
- [ ] Three points on `2026-09-01` (0.2), `2026-09-02` (0.4) and `2026-09-05` (0.1) with width 104, height 14 give `"M2,10 H27 V8 H102 V11"`.
- [ ] `<Sparkline points={[]} label="x" />` renders `No score history yet.`
- [ ] A leaderboard row with `trend: "worsening"` has an element with the accessible label `Worsening: score up by 0.05 or more versus 14 days ago`, and `trend: null` renders no trend mark.
- [ ] The Test drawer shows `Clean streak` with the Test's value, and an `img` role whose label starts with `Flakiness score,`. The Job drawer shows `clean streak 4` for a Job with `clean_streak: 4`.
- [ ] All frontend checks pass.

## Test Expectations
- Framework: Vitest + Testing Library. Run: `cd frontend && npm test`.
- `components/Sparkline.test.tsx` (new): the three `sparklinePath` literals above (fill the other `ScorePoint` fields with `0`), plus the empty-state render.
- `components/Leaderboard.test.tsx`: set the fixture's `trend: "worsening"` and assert `screen.getByLabelText(/^Worsening/)`. Render another with `trend: null` and assert `screen.queryByLabelText(/Worsening|Improving|Steady/)` is null.
- `components/TestDrawer.test.tsx`: fixture `clean_streak: 3` and `score_history: [{ day: "2026-09-20", flakiness_score: 0.6, confirmed_flake_count: 1, executions: 2, failures: 1 }]`. Assert `screen.getByText("Clean streak")`, `screen.getByText("3")` (or scope the query to the stat block if "3" collides with other text), and `screen.getByRole("img", { name: /^Flakiness score,/ })`.
- `components/JobDrawer.test.tsx`: fixture `clean_streak: 4` and `score_history: []`. Assert `screen.getByText(/clean streak 4/)` and `screen.getByText("No score history yet.")`.
- Add `clean_streak: 0, trend: null` to every other `TestCase`/`Job` fixture, and `score_history: []` to every other `History`/`JobHistory` fixture.

## Dependencies
- Blocked by: 06, 07.
- Why blocked: 06 adds `clean_streak`/`trend`/`score_history` for Tests, and 07 adds them for Jobs.
- Blocks: 09.

## Labels
`feature`, `frontend`, `priority:medium`

## Estimate
Medium

## Risk
1 - Display only. The path maths is unit-tested with literals.

## Validator Stopping Point
```bash
cd frontend && npm run typecheck && npm run lint && npm run format:check && npm test
```
