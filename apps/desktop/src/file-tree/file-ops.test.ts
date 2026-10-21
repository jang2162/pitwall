// 파일 조작의 순서(file-ops.ts, 12-m1-plan M1-21): 이름 검사, 옮기지 않을 이동, 휴지통 결과의 이유별 처리, Finder 드롭 결과 알림.
// 화면 · Host 를 붙인 흐름은 file-ops-window.test.tsx 가 본다.

import { ERROR_CODE, type FsTrashed } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import type { NotificationInput } from "../notifications/notifications";
import { fileOps, type FileOpsDeps, nameProblem } from "./file-ops";
import { fileTreeStrings } from "./strings";
import { FileTreeModel } from "./tree-model";

/** 문자열은 id 와 변수를 그대로 보인다. */
const T = fileTreeStrings((id, vars) => (vars ? `${id} ${JSON.stringify(vars)}` : id)).ops;

function setup(over: Partial<FileOpsDeps> = {}) {
  const notices: NotificationInput[] = [];
  const workspace = { moved: vi.fn(), deleted: vi.fn(), unsavedUnder: vi.fn(() => [] as string[]) };
  const model = new FileTreeModel({ list: async (path) => ({ path, entries: [] }) });
  const rpc = {
    create: vi.fn(async (path: string) => ({ path })),
    rename: vi.fn(async (from: string, to: string) => ({ from, to })),
    copy: vi.fn(async (_from: string, to: string) => ({
      to,
      files: 1,
      dirs: 0,
      links: 0,
      skipped: [],
    })),
    trash: vi.fn(async (paths: string[]): Promise<FsTrashed> => ({ removed: paths, failed: [] })),
  };
  const deps: FileOpsDeps = {
    rpc,
    workspace,
    model,
    strings: T,
    notify: (n) => notices.push(n),
    askName: vi.fn(async () => undefined),
    confirm: vi.fn(async () => true),
    open: vi.fn(),
    ...over,
  };
  return { ops: fileOps(deps), deps, rpc, workspace, notices };
}

describe("nameProblem", () => {
  it("refuses empty names, separators and the dot names", () => {
    expect(nameProblem("", T)).toBe(T.name.empty);
    expect(nameProblem("a/b", T)).toBe(T.name.separator);
    expect(nameProblem("a\\b", T)).toBe(T.name.separator);
    expect(nameProblem("..", T)).toBe(T.name.dots);
    expect(nameProblem(".", T)).toBe(T.name.dots);
    expect(nameProblem(".gitignore", T)).toBeUndefined();
    expect(nameProblem("새 파일.txt", T)).toBeUndefined();
  });
});

describe("move", () => {
  it("does not move a folder into itself or a file into the folder it is already in", async () => {
    const { ops, rpc } = setup();
    await ops.move(["/r/a"], "/r/a/b", false);
    await ops.move(["/r/a"], "/r/a", false);
    await ops.move(["/r/x.txt"], "/r", false);
    expect(rpc.rename).not.toHaveBeenCalled();
    await ops.move(["/r/x.txt"], "/r/a", false);
    expect(rpc.rename).toHaveBeenCalledWith("/r/x.txt", "/r/a/x.txt");
  });

  it("tells a taken name apart from other failures", async () => {
    const taken = Object.assign(new Error("exists"), { code: ERROR_CODE.CONFLICT });
    const { ops, notices, workspace } = setup({
      rpc: {
        ...setup().rpc,
        rename: vi.fn(async () => Promise.reject(taken)),
        copy: vi.fn(async () => Promise.reject(new Error("disk full"))),
      },
    });
    await ops.move(["/r/x.txt"], "/r/a", false);
    await ops.move(["/r/x.txt"], "/r/a", true);
    expect(notices.map((n) => n.title)).toEqual([T.exists("x.txt"), T.failed.copy("disk full")]);
    expect(workspace.moved).not.toHaveBeenCalled();
  });
});

describe("remove", () => {
  it("warns about unsaved tabs and does nothing when the confirmation is cancelled", async () => {
    const confirm = vi.fn(async () => false);
    const { ops, rpc, workspace } = setup({ confirm });
    workspace.unsavedUnder.mockReturnValue(["/r/dir/a.rs"]);
    await ops.remove(["/r/dir"]);
    expect(confirm).toHaveBeenCalledWith(
      expect.objectContaining({ warning: T.delete.unsaved("a.rs") }),
    );
    expect(rpc.trash).not.toHaveBeenCalled();
  });

  it("closes what went, reports permission failures and asks before a permanent delete", async () => {
    const trash = vi.fn(async (paths: string[], permanent: boolean): Promise<FsTrashed> =>
      permanent
        ? { removed: paths, failed: [] }
        : {
            removed: ["/r/a"],
            failed: [
              { path: "/r/gone", reason: "missing", message: "gone" },
              { path: "/r/locked", reason: "permission", message: "denied" },
              { path: "/r/usb/x", reason: "unavailable", message: "no trash on this volume" },
            ],
          },
    );
    const confirm = vi.fn(async () => true);
    const { ops, workspace, notices } = setup({
      rpc: { ...setup().rpc, trash },
      confirm,
    });
    await ops.remove(["/r/a", "/r/gone", "/r/locked", "/r/usb/x"]);
    expect(confirm).toHaveBeenLastCalledWith({
      title: T.permanent.title,
      body: T.permanent.body("x", "no trash on this volume"),
      confirm: T.permanent.confirm,
      danger: true,
    });
    expect(trash.mock.calls).toEqual([
      [["/r/a", "/r/gone", "/r/locked", "/r/usb/x"], false],
      [["/r/usb/x"], true],
    ]);
    expect(workspace.deleted.mock.calls.map((c) => c[0])).toEqual(["/r/a", "/r/gone", "/r/usb/x"]);
    expect(notices.map((n) => n.title)).toEqual([T.deletePermission("locked")]);
  });
});

describe("importDrop", () => {
  it("reports taken names, skipped special files and a cancelled copy", async () => {
    const importDrop = vi.fn(async () => ({
      copied: ["/r/a.txt"],
      conflicts: ["/Users/me/b.txt"],
      skipped: ["/Users/me/dir/pipe"],
      links: 1,
      cancelled: true,
    }));
    const { ops, notices } = setup({ importDrop });
    await ops.importDrop(3, "/r");
    expect(importDrop).toHaveBeenCalledWith(3, "/r");
    expect(notices.map((n) => [n.level, n.title])).toEqual([
      ["error", T.drop.conflicts("b.txt")],
      ["warning", T.drop.skipped("pipe")],
      ["info", T.drop.cancelled],
    ]);
  });
});
