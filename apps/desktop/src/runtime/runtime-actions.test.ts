import { describe, expect, it, vi } from "vitest";

import { KO_APP } from "../i18n/testing";
import { runtimeActions } from "./runtime-actions";

describe("runtimeActions", () => {
  it("reloads the runtime environment", async () => {
    const reload = vi.fn(async () => {});
    const action = runtimeActions({ strings: KO_APP.actions, reload })[0]!;
    expect(action.id).toBe("runtime.reload");
    expect(action.label).toBe("런타임 환경 다시 읽기");
    await action.run();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("logs a failed reload instead of throwing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const action = runtimeActions({
      strings: KO_APP.actions,
      reload: async () => {
        throw new Error("boom");
      },
    })[0]!;
    await expect(action.run()).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
