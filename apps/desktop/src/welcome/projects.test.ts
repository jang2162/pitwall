import type { Project, Resolution } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  baseName,
  candidateReasons,
  existingProjectFor,
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
