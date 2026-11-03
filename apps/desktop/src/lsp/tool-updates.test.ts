// @vitest-environment happy-dom
// 도구 업데이트 알림의 「하루 한 번」(15-m4-plan M4-17 ⓗ · 리뷰 M4): 부르기 전에 시각을 적어 자리를 잡으므로 창 여럿이 함께 떠도 한
// 번만 묻고 알린다. 저장소를 못 쓰면 창 안의 기억으로 하루 한 번이다. 부르기가 실패하면 자리를 되돌려 다음에 다시 묻는다.

import { describe, expect, it, vi } from "vitest";

import { bundledTranslator } from "../i18n/testing";
import { toolsStrings } from "../tools/strings";
import { checkToolUpdates, UPDATES_CHECKED_KEY, UPDATES_INTERVAL_MS } from "./lsp-status";

const T = toolsStrings(bundledTranslator("en"));
const HOUR = 60 * 60 * 1000;
const UPDATES = {
  items: [
    { name: "ruff", installed: "1", available: "2", confirmRequired: false, checksum: "pinned" },
  ],
  registryVersion: "r1",
};

function memoryStorage() {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
  };
}

const blocked = {
  getItem: (): string | null => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
  removeItem: () => {
    throw new Error("blocked");
  },
};

function deps(call: (method: string) => Promise<unknown>) {
  const notify = vi.fn();
  return {
    notify,
    base: {
      call: call as never,
      notify,
      strings: () => T,
      openTools: () => {},
    },
  };
}

describe("checkToolUpdates", () => {
  it("asks once when two windows check at the same time", async () => {
    const storage = memoryStorage();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const call = vi.fn(async () => {
      await gate;
      return UPDATES;
    });
    // 창마다 따로인 기억(웹뷰가 다르다) — 둘이 함께 보는 것은 저장소뿐이다.
    const a = deps(call);
    const b = deps(call);
    const now = 10 * UPDATES_INTERVAL_MS;
    const first = checkToolUpdates({ ...a.base, now, storage, memory: { at: 0 } });
    const second = checkToolUpdates({ ...b.base, now: now + 5, storage, memory: { at: 0 } });
    release();
    await Promise.all([first, second]);
    expect(call).toHaveBeenCalledTimes(1);
    expect(a.notify.mock.calls.length + b.notify.mock.calls.length).toBe(1);
  });

  it("tells once a day in a window whose storage is blocked", async () => {
    const call = vi.fn(async () => UPDATES);
    const d = deps(call);
    const memory = { at: 0 };
    const start = 20 * UPDATES_INTERVAL_MS;
    for (const at of [start, start + HOUR, start + 2 * HOUR])
      await checkToolUpdates({ ...d.base, now: at, storage: blocked, memory });
    expect(call).toHaveBeenCalledTimes(1);
    expect(d.notify).toHaveBeenCalledTimes(1);
    // 하루가 지나면 다시 묻는다.
    await checkToolUpdates({
      ...d.base,
      now: start + UPDATES_INTERVAL_MS,
      storage: blocked,
      memory,
    });
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("gives the day back when the check fails", async () => {
    const storage = memoryStorage();
    const memory = { at: 0 };
    const failing = deps(async () => {
      throw new Error("offline");
    });
    const now = 30 * UPDATES_INTERVAL_MS;
    await expect(checkToolUpdates({ ...failing.base, now, storage, memory })).rejects.toThrow(
      "offline",
    );
    expect(storage.map.has(UPDATES_CHECKED_KEY)).toBe(false);
    expect(memory.at).toBe(0);
    // 앞서 적힌 값이 있었으면 그 값으로 돌린다.
    storage.setItem(UPDATES_CHECKED_KEY, String(now - 2 * UPDATES_INTERVAL_MS));
    await expect(checkToolUpdates({ ...failing.base, now, storage, memory })).rejects.toThrow(
      "offline",
    );
    expect(storage.getItem(UPDATES_CHECKED_KEY)).toBe(String(now - 2 * UPDATES_INTERVAL_MS));
    const ok = deps(async () => UPDATES);
    await checkToolUpdates({ ...ok.base, now: now + 1, storage, memory });
    expect(ok.notify).toHaveBeenCalledTimes(1);
    expect(storage.getItem(UPDATES_CHECKED_KEY)).toBe(String(now + 1));
  });
});
