// 도구 설치 · 업데이트의 확인 흐름(15-m4-plan M4-17, 결정 M4-D2 「처음 한 번」 · M4-D17). 설치기(crates/pw-tools, M4-04)는 확인
// 기록이 없거나 라이선스 · 출처 · 고정 해시 유무가 바뀐 판이면 받지 않고 `4001`(`ToolConfirmRequired`)로 답한다. 창은 그 값을
// 확인 다이얼로그로 보이고, 사람이 [설치] · [업데이트] 를 누른 때만 그 값(`confirmed`)을 실어 다시 부른다 — `confirmed` 는 다이얼로그
// 를 지난 값만이다. 다이얼로그를 띄운 사이 판 · 라이선스 · 출처가 바뀌었으면 Host 가 다시 `4001`(`rejected: "changed"`)이고 새 값으로
// 다시 묻는다. [취소] 면 부르지 않는다.
//
// registry 에서 온 글(라이선스 · 주소 · 설명)은 미신뢰다 — 글로만 그리고(HTML 로 해석하지 않는다) 링크는 http(s) 만 연다(`safeUrl`).

import {
  RpcCallError,
  type ToolConfirmRequired,
  type ToolConfirmed,
  type ToolInstallResult,
  type UnaryMethods,
} from "@pitwall/rpc-client";

/** 설치기의 오류 코드(crates/pw-tools/src/install/mod.rs 의 `code` — tools.test.ts 가 그 파일을 읽어 값이 같은지 본다). */
export const TOOLS_ERROR = {
  CONFIRM_REQUIRED: 4001,
  RUNTIME_MISSING: 4002,
  INSTALL_FAILED: 4003,
  UNSUPPORTED: 4004,
  BUSY: 4005,
  NOT_INSTALLED: 4006,
  CANCELLED: 4007,
} as const;

/** 창의 Host 호출(설정 창 · 프로젝트 창). */
export type ToolsCall = <K extends keyof UnaryMethods & string>(
  method: K,
  params: UnaryMethods[K]["params"],
) => Promise<UnaryMethods[K]["result"]>;

export type ToolChangeKind = "install" | "update";

/** 확인 다이얼로그를 띄운다. 사람이 받아들이면 참. */
export type ToolConfirmAsk = (
  request: ToolConfirmRequired,
  kind: ToolChangeKind,
) => Promise<boolean>;

/** 같은 패키지에 확인이 몇 번까지 바뀌어도 다시 묻는가(그 뒤는 실패로 — 무한히 묻지 않는다). */
const MAX_ASKS = 4;

/**
 * 설치(`tools.install`) · 업데이트(`tools.update`)를 확인 흐름으로 부른다. 사람이 취소하면 `undefined`. 그 밖의 실패는 그대로
 * 던진다(`RpcCallError` — 4002 런타임 없음 · 4003 설치 실패 · 4007 취소 등).
 */
export async function changeTool(
  call: ToolsCall,
  ask: ToolConfirmAsk,
  kind: ToolChangeKind,
  name: string,
): Promise<ToolInstallResult | undefined> {
  let confirmed: ToolConfirmed | null = null;
  for (let asked = 0; ; asked++) {
    try {
      return kind === "install"
        ? await call("tools.install", { name, version: null, confirmed })
        : await call("tools.update", { name, confirmed });
    } catch (e) {
      if (!(e instanceof RpcCallError) || e.code !== TOOLS_ERROR.CONFIRM_REQUIRED) throw e;
      if (asked >= MAX_ASKS) throw e;
      const request = e.data as ToolConfirmRequired;
      if (!(await ask(request, kind))) return undefined;
      confirmed = {
        version: request.version,
        licenses: [...request.licenses],
        sourceDomains: [...request.sourceDomains],
      };
    }
  }
}

/** http(s) 주소만 돌려준다(그 밖의 스킴 · 깨진 주소는 링크로 만들지 않는다). */
export function safeUrl(url: string | null | undefined): string | undefined {
  if (!url) return undefined;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.href : undefined;
  } catch {
    return undefined;
  }
}

/** SPDX 식별자 하나의 목록 주소. `LicenseRef-…` 처럼 SPDX 목록에 없는 것 · 식이 섞인 글은 링크를 만들지 않는다. */
export function licenseUrl(id: string): string | undefined {
  if (!/^[A-Za-z0-9][A-Za-z0-9.+-]*$/.test(id) || id.startsWith("LicenseRef-")) return undefined;
  return `https://spdx.org/licenses/${id}.html`;
}

/** 사람이 읽는 크기(1000 단위가 아니라 1024 — 파일 크기 표기). */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/**
 * 서버 런타임(`runtimeMissing` 의 `tool` · `needed`)의 사람 이름과 안내할 mise 명령. Pitwall 은 런타임을 설치하지 않는다(D26 ·
 * 결정 M4-D6) — 명령은 사람이 고른다. 서버용이라 전역(`-g` 없이 `install`)만 권하고 프로젝트 고정(mise.toml)을 바꾸지 않는다.
 */
export function runtimeHint(tool: string, needed: string): { name: string; install: string } {
  const version = /\d+(?:\.\d+)*/.exec(needed)?.[0];
  const name =
    tool === "java" ? "JDK" : tool === "node" ? "Node" : tool === "python" ? "Python" : tool;
  return { name, install: version ? `mise install ${tool}@${version}` : `mise install ${tool}` };
}
