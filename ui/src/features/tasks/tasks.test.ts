import { describe, expect, it } from "vitest";
import { mockCore } from "../../lib/mock";
import type { ProjectsAreas, TaskView } from "../../lib/api";

const call = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  mockCore.invoke<T>(cmd, args);

const titles = (ts: TaskView[]): (string | null)[] => ts.map((t) => t.title);

describe("dev-mock tasks buckets (§3.1)", () => {
  it("partitions seeded tasks across the four derived buckets", async () => {
    const [today, upcoming, anytime, someday] = await Promise.all([
      call<TaskView[]>("tasks_bucket", { bucket: "Today" }),
      call<TaskView[]>("tasks_bucket", { bucket: "Upcoming" }),
      call<TaskView[]>("tasks_bucket", { bucket: "Anytime" }),
      call<TaskView[]>("tasks_bucket", { bucket: "Someday" }),
    ]);

    expect(titles(today)).toContain("Review the M2 acceptance checklist");
    expect(titles(upcoming)).toContain("Prepare the beta release notes");
    expect(titles(anytime)).toContain("Wire the tray menu actions");
    expect(titles(someday)).toContain("Explore the Parakeet speech backend");

    // Buckets are disjoint queries: nothing "someday" leaks into the active views.
    for (const t of [...today, ...upcoming, ...anytime]) expect(t.someday).toBe(false);
    for (const t of someday) expect(t.someday).toBe(true);
  });

  it("set_status completes a task out of its open bucket", async () => {
    const created = await call<TaskView>("tasks_create", { input: { title: "Ship the calendar view" } });
    let anytime = await call<TaskView[]>("tasks_bucket", { bucket: "Anytime" });
    expect(anytime.map((t) => t.id)).toContain(created.id);

    const done = await call<TaskView>("tasks_set_status", { task_id: created.id, status: "completed" });
    expect(done.status).toBe("completed");
    expect(done.completed_at).not.toBeNull();

    anytime = await call<TaskView[]>("tasks_bucket", { bucket: "Anytime" });
    expect(anytime.map((t) => t.id)).not.toContain(created.id);
  });

  it("exposes the projects + areas the view groups by", async () => {
    const pa = await call<ProjectsAreas>("tasks_projects_areas", {});
    expect(pa.areas.length).toBeGreaterThanOrEqual(2);
    expect(pa.projects.length).toBeGreaterThanOrEqual(2);
    expect(pa.areas.map((a) => a.name)).toContain("Work");
    expect(pa.projects.map((p) => p.name)).toContain("Q3 Launch");
  });
});
