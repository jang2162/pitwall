import type { ProjectBundle } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { machinesByProject } from "./machines";

const bundle = (id: string, members: [string, string][], implicit = false): ProjectBundle => ({
  id,
  name: id,
  favorite: false,
  tags: [],
  openedAt: null,
  implicit,
  members: members.map(([machine, projectId]) => ({ machine, projectId, lastTarget: null })),
});

describe("machinesByProject", () => {
  it("maps a local project to its bundle's machines, and leaves single-machine bundles out", () => {
    const map = machinesByProject([
      bundle("p1", [["local", "p1"]], true),
      bundle("b-1", [
        ["local", "p2"],
        ["SHA256:mini", "r1"],
        ["SHA256:pc", "r2"],
      ]),
      // 원격만인 묶음은 로컬 프로젝트 줄이 없다.
      bundle("b-2", [["SHA256:mini", "r3"]]),
    ]);
    expect([...map]).toEqual([["p2", ["local", "SHA256:mini", "SHA256:pc"]]]);
  });
});
