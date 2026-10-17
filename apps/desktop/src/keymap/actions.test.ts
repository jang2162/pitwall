import { describe, expect, it, vi } from "vitest";

import { appActions } from "./actions";

const base = { openFindAction() {}, openSettings: async () => {}, setTheme() {} };

describe("appActions", () => {
  // src-tauri/src/menu.rs 의 CLI_INSTALL_ACTION 과 같은 id 여야 메뉴에서 고른 것이 이 액션으로 온다.
  it("registers the shell command action only where it can be installed", async () => {
    expect(appActions(base).map((a) => a.id)).not.toContain("cli.install");
    const install = vi.fn(async () => {});
    const action = appActions({ ...base, installShellCommand: install }).find(
      (a) => a.id === "cli.install",
    );
    expect(action).toBeDefined();
    await action!.run();
    expect(install).toHaveBeenCalledTimes(1);
  });

  it("swallows a cancelled or failed install (the native side already told the user)", async () => {
    const install = vi.fn(async () => {
      throw "cancelled";
    });
    const action = appActions({ ...base, installShellCommand: install }).find(
      (a) => a.id === "cli.install",
    )!;
    await expect(action.run()).resolves.toBeUndefined();
  });
});
