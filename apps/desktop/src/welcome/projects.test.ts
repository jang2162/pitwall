import type { Project, Resolution } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  baseName,
  candidateReasons,
  existingProjectFor,
  openArgsFor,
  parentDir,
  sortProjects,
  storageBanner,
} from "./projects";

const p = (id: string, extra: Partial<Project> = {}): Project => ({
  id,
  name: id,
  host: "local",
  roots: [`/w/${id}`],
  favorite: false,
  tags: [],
  configMode: "repository",
  slug: id,
  createdAt: 0,
  openedAt: null,
  ...extra,
});

describe("sortProjects", () => {
  it("puts favorites first, then the most recently opened, then never-opened by name", () => {
    const sorted = sortProjects([
      p("c"),
      p("old", { openedAt: 1 }),
      p("a"),
      p("fav-old", { favorite: true, openedAt: 2 }),
      p("new", { openedAt: 9 }),
      p("fav-never", { favorite: true }),
    ]);
    expect(sorted.map((x) => x.id)).toEqual(["fav-old", "fav-never", "new", "old", "a", "c"]);
  });
});

describe("existingProjectFor", () => {
  const project = p("app", { roots: ["/w/app"] });
  const at = (path: string, worktree: Resolution["worktree"] = null): Resolution => ({
    project,
    root: "/w/app",
    path,
    worktree,
  });

  it("opens the project whose root is the folder", () => {
    expect(existingProjectFor(at("/w/app"))).toEqual({ projectId: "app" });
  });

  it("does not open a project for a folder inside it — that folder becomes a new project", () => {
    expect(existingProjectFor(at("/w/app/sub"))).toBeUndefined();
    expect(existingProjectFor(null)).toBeUndefined();
  });

  it("opens an unregistered worktree's folder that matches the root as that worktree", () => {
    const worktree = { top: "/w/app-wt", root: "/w/app-wt" };
    expect(existingProjectFor(at("/w/app-wt", worktree))).toEqual({
      projectId: "app",
      worktree: "/w/app-wt",
    });
    expect(existingProjectFor(at("/w/app-wt/src", worktree))).toBeUndefined();
  });
});

describe("paths", () => {
  it("splits POSIX and Windows paths", () => {
    expect(baseName("/w/billing-api/")).toBe("billing-api");
    expect(baseName("C:\\work\\web")).toBe("web");
    expect(parentDir("/w/app/src/App.kt")).toBe("/w/app/src");
    expect(parentDir("C:\\work\\web\\a.ts")).toBe("C:\\work\\web");
    expect(parentDir("/a")).toBe("/a");
  });
});

it("lists candidate reasons in the 10-ui §2.2 order", () => {
  expect(
    candidateReasons({ path: "/w", worktreeMain: true, vcs: true, buildFiles: ["Cargo.toml"] }),
  ).toBe("워크트리의 메인 저장소 · .git · Cargo.toml");
});

it("shows a banner for every non-ok registry and blocks writes only when they are rejected", () => {
  expect(storageBanner({ kind: "ok", path: "/s" })).toBeUndefined();
  expect(storageBanner(undefined)).toBeUndefined();
  expect(
    storageBanner({ kind: "recovered", path: "/s", backup: "/s.bak", reason: "corrupt" }),
  ).toMatchObject({ readOnly: false, text: expect.stringContaining("/s.bak") });
  expect(storageBanner({ kind: "readOnly", path: "/s", reason: "newer" })?.readOnly).toBe(true);
  expect(storageBanner({ kind: "memory", path: "/s", reason: "locked" })?.readOnly).toBe(true);
});

describe("openArgsFor", () => {
  const app = p("app", { roots: ["/w/app"] });
  const at = (path: string, worktree: Resolution["worktree"] = null): Resolution => ({
    project: app,
    root: "/w/app",
    path,
    worktree,
  });

  it("passes the file with its line and column when the project holds it", () => {
    expect(
      openArgsFor(at("/w/app/src/a.ts"), "app", { path: "/w/app/src/a.ts", line: 3, col: null }),
    ).toEqual({
      projectId: "app",
      file: "/w/app/src/a.ts",
      line: 3,
    });
  });

  it("opens a worktree at its top and passes the file inside it", () => {
    const worktree = { top: "/w/wt", root: "/w/wt/svc" };
    expect(openArgsFor(at("/w/wt/svc/a.ts", worktree), "app", { path: "/w/wt/svc/a.ts" })).toEqual({
      projectId: "app",
      worktree: "/w/wt",
      file: "/w/wt/svc/a.ts",
    });
  });

  it("does not pass the project root, the worktree top or the worktree root as a file", () => {
    const worktree = { top: "/w/wt", root: "/w/wt/svc" };
    for (const path of ["/w/wt", "/w/wt/svc"]) {
      expect(openArgsFor(at(path, worktree), "app", { path, line: 1 })).toEqual({
        projectId: "app",
        worktree: "/w/wt",
      });
    }
    expect(openArgsFor(at("/w/app"), "app", { path: "/w/app", line: 1 })).toEqual({
      projectId: "app",
    });
  });

  it("gives nothing when the path is in another project or in none", () => {
    expect(openArgsFor(at("/w/app/a.ts"), "other", { path: "/w/app/a.ts" })).toBeUndefined();
    expect(openArgsFor(null, "app", { path: "/x/a.ts" })).toBeUndefined();
  });
});
