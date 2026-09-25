import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { RepoInfo } from "../api";
import { ScopePicker } from "./ScopePicker";

const repos: RepoInfo[] = [
  { name: "andrewthetechie/fantasy", projects: [{ name: "backend", root: "" }] },
  {
    name: "andrewthetechie/writers-app",
    projects: [
      { name: "backend", root: "" },
      { name: "e2e", root: "e2e" },
    ],
  },
];

describe("ScopePicker", () => {
  it("disables the project picker until a repo is chosen", () => {
    render(<ScopePicker repos={repos} scope={{ repo: null, project: null }} onChange={() => {}} />);
    expect(screen.getByLabelText("Project")).toBeDisabled();
  });

  it("choosing a repo reports it with no project", async () => {
    const onChange = vi.fn();
    render(<ScopePicker repos={repos} scope={{ repo: null, project: null }} onChange={onChange} />);
    await userEvent.selectOptions(screen.getByLabelText("Repo"), "andrewthetechie/writers-app");
    expect(onChange).toHaveBeenCalledWith({ repo: "andrewthetechie/writers-app", project: null });
  });

  it("lists only the chosen repo's projects", async () => {
    const onChange = vi.fn();
    render(
      <ScopePicker
        repos={repos}
        scope={{ repo: "andrewthetechie/writers-app", project: null }}
        onChange={onChange}
      />,
    );
    const project = screen.getByLabelText("Project");
    expect(project).toBeEnabled();
    const options = Array.from(project.querySelectorAll("option")).map((o) => o.textContent);
    expect(options).toEqual(["All projects", "backend", "e2e"]);
    await userEvent.selectOptions(project, "e2e");
    expect(onChange).toHaveBeenCalledWith({ repo: "andrewthetechie/writers-app", project: "e2e" });
  });
});
