import { describe, expect, it, vi } from "vitest";

import { ActionRegistry } from "../src/registry";

describe("ActionRegistry 인자", () => {
  it("run 의 인자를 액션에 그대로 넘긴다 — 인자 없이 부르면 undefined", () => {
    const registry = new ActionRegistry();
    const run = vi.fn();
    registry.register({ id: "a.b", label: "a", run });
    expect(registry.run("a.b", { rev: "abc" })).toBe(true);
    expect(registry.run("a.b")).toBe(true);
    expect(run.mock.calls).toEqual([[{ rev: "abc" }], [undefined]]);
  });

  it("args 액션은 인자 없이 돌지 않고 isEnabled 가 거짓이다(키 · 액션 찾기가 가져가지 않는다)", () => {
    const registry = new ActionRegistry();
    const run = vi.fn();
    registry.register({ id: "git.showCommit", label: "show", args: true, run });
    expect(registry.isEnabled("git.showCommit")).toBe(false);
    expect(registry.needsArgs("git.showCommit")).toBe(true);
    expect(registry.run("git.showCommit")).toBe(false);
    expect(run).not.toHaveBeenCalled();
    expect(registry.run("git.showCommit", { rev: "HEAD" })).toBe(true);
    expect(run).toHaveBeenCalledWith({ rev: "HEAD" });
  });

  it("args 액션도 enabled 가 거짓이면 인자가 있어도 돌지 않는다", () => {
    const registry = new ActionRegistry();
    const run = vi.fn();
    registry.register({ id: "x", label: "x", args: true, enabled: () => false, run });
    expect(registry.run("x", {})).toBe(false);
    expect(run).not.toHaveBeenCalled();
  });
});
