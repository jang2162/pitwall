import type { Project, ProjectBundle } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { bundleRows, isWindowRow } from "./machines";

const bundle = (
  id: string,
  members: [string, string][],
  over: Partial<ProjectBundle> = {},
): ProjectBundle => ({
  id,
  name: id,
  favorite: false,
  tags: [],
  openedAt: null,
  implicit: false,
  members: members.map(([machine, projectId]) => ({ machine, projectId, lastTarget: null })),
  ...over,
});

const project = (id: string, over: Partial<Project> = {}): Project =>
  ({
    id,
    name: id,
    roots: [`/work/${id}`],
    favorite: false,
    tags: [],
    openedAt: null,
    ...over,
  }) as Project;

describe("bundleRows", () => {
  it("makes one row per bundle, keeps remote-only bundles, and marks machines only when there is another", () => {
    const rows = bundleRows(
      [
        bundle("p1", [["local", "p1"]], { implicit: true }),
        bundle("b-1", [
          ["local", "p2"],
          ["SHA256:mini", "r1"],
        ]),
        bundle("b-2", [["SHA256:mini", "r3"]], { name: "remote app" }),
      ],
      [project("p1"), project("p2")],
    );
    expect(rows.map((r) => [r.id, r.project?.id, r.machines])).toEqual([
      ["b-1", "p2", ["local", "SHA256:mini"]],
      ["p1", "p1", []],
      ["b-2", undefined, ["SHA256:mini"]],
    ]);
    expect(rows[2]!.name).toBe("remote app");
  });

  it("orders by favorite then last opened, taking the later of the bundle's and its local project's", () => {
    const rows = bundleRows(
      [
        bundle("b-1", [
          ["local", "p2"],
          ["m", "r1"],
        ]),
        bundle("b-2", [["m", "r3"]], { openedAt: 5 }),
        bundle("p1", [["local", "p1"]], { implicit: true }),
      ],
      [project("p1", { favorite: true }), project("p2", { openedAt: 9 })],
    );
    expect(rows.map((r) => r.id)).toEqual(["p1", "b-1", "b-2"]);
    expect(rows[1]!.openedAt).toBe(9);
  });

  it("falls back to one row per local project when bundles are not read, and keeps a project the bundle list missed", () => {
    expect(bundleRows(undefined, [project("a"), project("b")]).map((r) => r.id)).toEqual([
      "a",
      "b",
    ]);
    expect(
      bundleRows(
        [bundle("a", [["local", "a"]], { implicit: true })],
        [project("a"), project("n")],
      ).map((r) => r.id),
    ).toEqual(["a", "n"]);
  });

  it("knows a window by its bundle id or by its local project id", () => {
    const [row] = bundleRows(
      [
        bundle("b-1", [
          ["local", "p2"],
          ["m", "r1"],
        ]),
      ],
      [project("p2")],
    );
    expect(isWindowRow(row!, "b-1")).toBe(true);
    expect(isWindowRow(row!, "p2")).toBe(true);
    expect(isWindowRow(row!, "p3")).toBe(false);
    expect(isWindowRow(row!, undefined)).toBe(false);
  });
});
