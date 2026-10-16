// 기본 키맵 + 사용자 `$CONFIG/keymap.toml`(설정 계층의 `config.get` 결과) → 지금 쓸 바인딩과 알릴 오류.
//
// 사용자 파일이 틀리면 **이전 키맵을 유지**하고 오류를 알린다(10-ui §4.5 「파일 편집」). 틀리는 길은 둘이다.
// - TOML 문법 오류: 설정 계층이 읽기를 실패로 보고 이전 스냅숏을 그대로 준다(`ConfigView.error`). 스냅숏의 keymap 값이
//   곧 이전 것이므로 그것을 그대로 병합하고 오류만 알린다.
// - 형식 오류(모르는 키 이름 · 필드 등): 설정 계층은 성공이라 새 값이 온다. 여기서 읽다 실패하면 `previous` 를 쓴다.

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

export interface ResolvedKeymap {
  readonly bindings: readonly Binding[];
  /** 알릴 오류. 있으면 `bindings` 는 마지막으로 읽힌 키맵이다. */
  readonly error?: string;
}

function isKeymapFile(path: string | null | undefined): boolean {
  return path != null && /(^|[/\\])keymap(\.local)?\.toml$/.test(path);
}

export function resolveKeymap(
  base: readonly Binding[],
  config: ConfigView | undefined,
  previous: readonly Binding[] | undefined,
  platform: Platform,
): ResolvedKeymap {
  const user = config?.snapshot.files.find((f) => f.id === KEYMAP_CONFIG_ID)?.value;
  const fault = config?.error && isKeymapFile(config.error.path) ? config.error.message : undefined;
  try {
    const bindings = mergeKeymap(base, readUserKeymap(user, platform));
    return fault ? { bindings, error: fault } : { bindings };
  } catch (e) {
    if (!(e instanceof KeymapFileError)) throw e;
    // 문법 오류가 있으면 그것이 지금 파일의 문제다 — 스냅숏의 형식 오류는 그 전 내용의 것이다.
    return { bindings: previous ?? base, error: fault ?? `keymap.toml: ${e.problems.join("; ")}` };
  }
}
