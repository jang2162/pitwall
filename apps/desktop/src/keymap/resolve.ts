// 기본 키맵 + 사용자 `$CONFIG/keymap.toml`(설정 계층의 `config.get` 결과) → 지금 쓸 바인딩과 알릴 오류.
//
// 사용자 파일이 틀리면 **마지막으로 읽힌 키맵을 유지**하고 오류를 알린다(10-ui §4.5 「파일 편집」). 틀리는 길은 둘이다.
// - TOML 문법 오류: 설정 계층이 읽기를 실패로 보고 이전 스냅숏을 그대로 준다(`ConfigView.error`). 앱을 띄울 때부터
//   틀렸으면 그 스냅숏은 내장 기본값뿐이다.
// - 형식 오류(모르는 키 이름 · 필드 등): 설정 계층은 성공이라 새 값이 온다. 여기서 읽다 실패한다.
// 「마지막으로 읽힌 키맵」은 이 창의 직전 결과(`previous`), 없으면 앱 전체가 함께 쓰는 마지막 성공값(`lastGood`,
// keymap-provider.tsx 가 localStorage 에 둔다)이다. 창마다 따로 두면 오류 상태에서 새로 연 창만 기본 키맵을 쓰고, 그 창의
// 메뉴 동기화가 메뉴를 기본값으로 되돌린다.

import {
  type Binding,
  KeymapFileError,
  mergeKeymap,
  type Platform,
  readUserKeymap,
} from "@pitwall/keymap";
import type { ConfigView } from "@pitwall/rpc-client";

/** 설정 계층의 파일 id(crates/pw-core/src/config/source.rs — 설정 폴더 기준 경로에서 `.toml` 을 뗀 것). */
export const KEYMAP_CONFIG_ID = "keymap";

/** 마지막으로 읽힌 사용자 키맵 값(파일이 없었으면 `value` 가 `null`). */
export interface LastGoodKeymap {
  readonly value: unknown;
}

export interface ResolvedKeymap {
  readonly bindings: readonly Binding[];
  /** 알릴 오류. 있으면 `bindings` 는 마지막으로 읽힌 키맵이다. */
  readonly error?: string;
  /** 막지는 않은 문제(그 OS 에 없는 키 등). */
  readonly warnings: readonly string[];
  /** 오류 없이 읽혔으면 그 값 — 호출자가 `lastGood` 로 남긴다. */
  readonly good?: LastGoodKeymap;
}

function isKeymapFile(path: string | null | undefined): boolean {
  return path != null && /(^|[/\\])keymap(\.local)?\.toml$/.test(path);
}

function mergeLastGood(
  base: readonly Binding[],
  lastGood: LastGoodKeymap | undefined,
  platform: Platform,
): Binding[] | undefined {
  if (!lastGood) return undefined;
  try {
    return mergeKeymap(base, readUserKeymap(lastGood.value ?? undefined, platform));
  } catch {
    return undefined;
  }
}

/** 이 창이 직전에 쓴 바인딩과, 그것을 만든 기본 키맵. */
export interface PreviousKeymap {
  readonly base: readonly Binding[];
  readonly bindings: readonly Binding[];
}

export function resolveKeymap(
  base: readonly Binding[],
  config: ConfigView | undefined,
  previous: PreviousKeymap | undefined,
  platform: Platform,
  lastGood?: LastGoodKeymap,
): ResolvedKeymap {
  const user = config?.snapshot.files.find((f) => f.id === KEYMAP_CONFIG_ID)?.value;
  const fault = config?.error && isKeymapFile(config.error.path) ? config.error.message : undefined;
  // 기본 키맵이 바뀌었으면(번들 → 확장 레지스트리, default-keymap.ts) 직전 바인딩은 옛 기본 키맵 위의 것이다. 그때는
  // 마지막 성공값을 새 기본 키맵 위에 다시 병합한다 — 직전 것을 그대로 두면 사용자 파일을 고칠 때까지 새 기본 키맵이
  // 적용되지 않는다.
  const kept = () =>
    (previous?.base === base ? previous.bindings : undefined) ??
    mergeLastGood(base, lastGood, platform);
  // 설정을 아직 받지 못했다(창이 막 뜸). 「파일 없음」이 아니므로 기본 키맵으로 메뉴를 되돌리지 않고, 마지막 성공값도 덮지 않는다.
  if (!config) return { bindings: kept() ?? base, warnings: [] };
  try {
    const keymap = readUserKeymap(user, platform);
    const bindings = mergeKeymap(base, keymap);
    if (fault) return { bindings: kept() ?? bindings, error: fault, warnings: keymap.warnings };
    return { bindings, warnings: keymap.warnings, good: { value: user ?? null } };
  } catch (e) {
    if (!(e instanceof KeymapFileError)) throw e;
    // 문법 오류가 있으면 그것이 지금 파일의 문제다 — 스냅숏의 형식 오류는 그 전 내용의 것이다.
    return {
      bindings: kept() ?? base,
      error: fault ?? `keymap.toml: ${e.problems.join("; ")}`,
      warnings: [],
    };
  }
}
