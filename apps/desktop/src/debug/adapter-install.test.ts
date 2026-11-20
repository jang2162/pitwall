// 디버그 어댑터 설치로 잇기(adapter-install.ts)의 거르기와 한 번만 다시 시작하기. 설치로 고칠 수 없는 오류(경로 설정 잘못 ·
// 설치 패키지 없음 · 꺼짐 · 런타임 없음 · 다른 코드)는 설치 확인으로 잇지 않는다.

import { ERROR_CODE, type JsonValue, RpcCallError } from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { missingAdapterOf, startOrInstallAdapter } from "./adapter-install";

const notInstalled = {
  kind: "notInstalled",
  mason: "js-debug-adapter",
  reinstall: null,
  skipped: [],
};

function error(data: JsonValue, code: number = ERROR_CODE.INVALID_PARAMS) {
  return new RpcCallError({ code, message: "the debug adapter `js-debug` is not installed", data });
}

const missing = (resolution: JsonValue) =>
  error({ reason: "adapterMissing", adapter: "js-debug", resolution });

it("offers an install only for an adapter that is not installed and has a package", () => {
  expect(missingAdapterOf(missing(notInstalled))).toEqual({
    adapter: "js-debug",
    mason: "js-debug-adapter",
  });
  // 사람이 적은 경로가 깨짐 — 설치해도 고쳐지지 않는다(패키지가 실려 와도).
  expect(
    missingAdapterOf(
      missing({
        kind: "invalidPath",
        path: "/nowhere",
        origin: "global",
        reason: "not found",
        mason: "js-debug-adapter",
      }),
    ),
  ).toBeUndefined();
  expect(missingAdapterOf(missing({ ...notInstalled, mason: null }))).toBeUndefined();
  expect(missingAdapterOf(missing({ ...notInstalled, mason: "" }))).toBeUndefined();
  expect(missingAdapterOf(missing(null))).toBeUndefined();
  for (const reason of ["adapterDisabled", "runtimeMissing", "noDebug"])
    expect(
      missingAdapterOf(error({ reason, adapter: "js-debug", resolution: notInstalled })),
    ).toBeUndefined();
  expect(
    missingAdapterOf(
      error(
        { reason: "adapterMissing", adapter: "js-debug", resolution: notInstalled },
        ERROR_CODE.INTERNAL_ERROR,
      ),
    ),
  ).toBeUndefined();
  expect(missingAdapterOf(new Error("adapterMissing"))).toBeUndefined();
});

it("starts once more after the install and throws a second failure without installing again", async () => {
  const second = missing(notInstalled);
  const start = vi
    .fn<() => Promise<string>>()
    .mockRejectedValueOnce(missing(notInstalled))
    .mockRejectedValueOnce(second);
  const install = vi.fn(async () => true);
  await expect(startOrInstallAdapter(start, install)).rejects.toBe(second);
  expect(start).toHaveBeenCalledTimes(2);
  expect(install).toHaveBeenCalledTimes(1);
  expect(install).toHaveBeenCalledWith({ adapter: "js-debug", mason: "js-debug-adapter" });
});

it("neither installs nor starts again when the error cannot be fixed by an install", async () => {
  const broken = missing({ kind: "invalidPath", path: "/x", origin: "global", reason: "gone" });
  const start = vi.fn<() => Promise<string>>().mockRejectedValue(broken);
  const install = vi.fn(async () => true);
  await expect(startOrInstallAdapter(start, install)).rejects.toBe(broken);
  expect(start).toHaveBeenCalledTimes(1);
  expect(install).not.toHaveBeenCalled();
});
