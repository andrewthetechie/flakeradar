import type { RepoInfo, Scope } from "../api";

/** Repo picker, then Project picker (enabled once a Repo is chosen:
 *  project names like "backend" repeat across repos). */
export function ScopePicker({
  repos, scope, onChange,
}: {
  repos: RepoInfo[];
  scope: Scope;
  onChange: (scope: Scope) => void;
}) {
  const projects = repos.find((r) => r.name === scope.repo)?.projects ?? [];
  return (
    <div className="scope-picker">
      <select
        aria-label="Repo"
        value={scope.repo ?? ""}
        onChange={(e) => onChange({ repo: e.target.value || null, project: null })}
      >
        <option value="">All repos</option>
        {repos.map((r) => <option key={r.name} value={r.name}>{r.name}</option>)}
      </select>
      <select
        aria-label="Project"
        value={scope.project ?? ""}
        disabled={!scope.repo}
        onChange={(e) => onChange({ repo: scope.repo, project: e.target.value || null })}
      >
        <option value="">All projects</option>
        {projects.map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
      </select>
    </div>
  );
}
