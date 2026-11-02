// 확인 흐름(15-m4-plan M4-17): 다이얼로그를 지나지 않은 `confirmed` 는 Host 에 가지 않고, 취소하면 다시 부르지 않고, 다이얼로그를 띄운
// 사이 바뀐 값(`rejected: "changed"`) · 라이선스가 바뀐 업데이트는 다시 묻는다. 링크는 http(s) 만, SPDX 목록 링크는 식별자 하나만.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  RpcCallError,
  type ToolConfirmRequired,
  type ToolInstallResult,
} from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  changeTool,
  formatBytes,
  licenseUrl,
  runtimeHint,
  safeUrl,
  TOOLS_ERROR,
  type ToolsCall,
} from "./install";

function request(over: Partial<ToolConfirmRequired> = {}): ToolConfirmRequired {
  return {
    name: "basedpyright",
    version: "1.40.2",
    licenses: ["MIT"],
    sourceDomains: ["files.pythonhosted.org", "pypi.org"],
    size: null,
    checksum: "pinned",
    note: null,
    rejected: null,
    ...over,
  };
}

const installed = (version: string): ToolInstallResult => ({
  name: "basedpyright",
  version,
  previous: null,
  outcome: "installed",
  checksum: "pinned",
  taskId: 1,
  joined: false,
});

/** 가짜 설치기: 확인 값이 `wanted` 와 같을 때만 설치한다(crates/pw-tools 의 `covers`). */
function fakeInstaller(wanted: () => ToolConfirmRequired) {
  const calls: { method: string; params: unknown }[] = [];
  const call = (async (method: string, params: { confirmed?: unknown }) => {
    calls.push({ method, params });
    const want = wanted();
    const c = params.confirmed as
      | { version: string; licenses: string[]; sourceDomains: string[] }
      | null
      | undefined;
    const covered =
      c &&
      c.version === want.version &&
      JSON.stringify(c.licenses) === JSON.stringify(want.licenses) &&
      JSON.stringify(c.sourceDomains) === JSON.stringify(want.sourceDomains);
    if (!covered)
      throw new RpcCallError({
        code: TOOLS_ERROR.CONFIRM_REQUIRED,
        message: "confirm",
        data: { ...want, rejected: c ? "changed" : null },
      });
    return installed(want.version);
  }) as unknown as ToolsCall;
  return { call, calls };
}

describe("changeTool", () => {
  it("sends a confirmed value only after the dialog accepted it", async () => {
    const { call, calls } = fakeInstaller(() => request());
    const asked: ToolConfirmRequired[] = [];
    const done = await changeTool(
      call,
      async (r) => {
        asked.push(r);
        // 다이얼로그가 떠 있는 동안에는 확인 값을 실은 호출이 없다.
        expect(calls.filter((c) => (c.params as { confirmed: unknown }).confirmed)).toEqual([]);
        return true;
      },
      "install",
      "basedpyright",
    );
    expect(done?.version).toBe("1.40.2");
    expect(asked).toHaveLength(1);
    expect(calls.map((c) => [c.method, (c.params as { confirmed: unknown }).confirmed])).toEqual([
      ["tools.install", null],
      [
        "tools.install",
        {
          version: "1.40.2",
          licenses: ["MIT"],
          sourceDomains: ["files.pythonhosted.org", "pypi.org"],
        },
      ],
    ]);
  });

  it("calls nothing more when the dialog is cancelled", async () => {
    const { call, calls } = fakeInstaller(() => request());
    expect(await changeTool(call, async () => false, "install", "basedpyright")).toBeUndefined();
    expect(calls).toHaveLength(1);
    expect((calls[0]!.params as { confirmed: unknown }).confirmed).toBeNull();
  });

  it("asks again with the new values when they changed behind the dialog", async () => {
    let want = request();
    const { call, calls } = fakeInstaller(() => want);
    const asked: ToolConfirmRequired[] = [];
    await changeTool(
      call,
      async (r) => {
        asked.push(r);
        // 처음 다이얼로그를 띄운 사이 registry 가 라이선스를 바꿨다.
        if (asked.length === 1) want = request({ licenses: ["MIT", "Apache-2.0"] });
        return true;
      },
      "install",
      "basedpyright",
    );
    expect(asked.map((r) => [r.licenses, r.rejected])).toEqual([
      [["MIT"], null],
      [["MIT", "Apache-2.0"], "changed"],
    ]);
    expect(calls).toHaveLength(3);
  });

  it("asks before an update whose license changed and not before one that did not", async () => {
    const asked: string[] = [];
    // 확인 기록이 덮는 판(판만 바뀜): 설치기가 바로 설치한다.
    const quiet = (async (method: string) => {
      asked.push(`call ${method}`);
      return installed("2.0.0");
    }) as unknown as ToolsCall;
    await changeTool(quiet, async () => (asked.push("ask"), true), "update", "basedpyright");
    expect(asked).toEqual(["call tools.update"]);
    asked.length = 0;
    const { call, calls } = fakeInstaller(() =>
      request({ version: "2.0.0", licenses: ["GPL-3.0"] }),
    );
    await changeTool(
      call,
      async (r) => (asked.push(`ask ${r.licenses}`), true),
      "update",
      "basedpyright",
    );
    expect(asked).toEqual(["ask GPL-3.0"]);
    expect(calls.map((c) => c.method)).toEqual(["tools.update", "tools.update"]);
  });

  it("passes other failures through", async () => {
    const call = (async () => {
      throw new RpcCallError({
        code: TOOLS_ERROR.RUNTIME_MISSING,
        message: "node",
        data: { runtime: "node" },
      });
    }) as unknown as ToolsCall;
    await expect(changeTool(call, async () => true, "install", "x")).rejects.toMatchObject({
      code: TOOLS_ERROR.RUNTIME_MISSING,
    });
  });
});

describe("links and text", () => {
  it("links only http(s) addresses", () => {
    expect(safeUrl("https://github.com/x")).toBe("https://github.com/x");
    expect(safeUrl("http://example.com/")).toBe("http://example.com/");
    for (const bad of [
      "javascript:alert(1)",
      "file:///etc/passwd",
      "data:text/html,x",
      "/relative",
      "",
      null,
    ])
      expect(safeUrl(bad)).toBeUndefined();
  });

  it("links SPDX identifiers and leaves LicenseRef and expressions as text", () => {
    expect(licenseUrl("MIT")).toBe("https://spdx.org/licenses/MIT.html");
    expect(licenseUrl("EPL-2.0")).toBe("https://spdx.org/licenses/EPL-2.0.html");
    expect(licenseUrl("LicenseRef-JetBrains-proprietary-components")).toBeUndefined();
    expect(licenseUrl("MIT OR Apache-2.0")).toBeUndefined();
    expect(licenseUrl("../x")).toBeUndefined();
  });

  it("names the server runtime and the mise command without touching the project pin", () => {
    expect(runtimeHint("java", ">=21")).toEqual({ name: "JDK", install: "mise install java@21" });
    expect(runtimeHint("python", ">=3.10")).toEqual({
      name: "Python",
      install: "mise install python@3.10",
    });
    expect(runtimeHint("node", "*").install).toBe("mise install node");
  });

  it("formats sizes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(13.7 * 1024 * 1024)).toBe("14 MiB");
    expect(formatBytes(1.5 * 1024 * 1024)).toBe("1.5 MiB");
  });

  it("keeps the installer's error codes (crates/pw-tools/src/install/mod.rs)", () => {
    const source = readFileSync(
      resolve(__dirname, "../../../../crates/pw-tools/src/install/mod.rs"),
      "utf8",
    );
    for (const [name, value] of Object.entries(TOOLS_ERROR))
      expect(source).toContain(`pub const ${name}: i32 = ${value};`);
  });
});
