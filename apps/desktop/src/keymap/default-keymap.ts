// 기본 키맵: 내장 확장 keymap-default 의 기여를 확장 레지스트리(crates/pw-ext 의 `keymap.get`)에서 받는다.
//
// 번들에도 같은 파일을 묶어 레지스트리 응답 전의 첫 입력에 쓴다 — 응답을 기다리는 동안 단축키 · 메뉴 accelerator 가
// 비지 않게 한다. 응답이 오면 그 키맵으로 바꾸고, 못 받거나 읽을 수 없으면 번들 것을 그대로 쓴다. 테마와 같은 방식이다
// (docs/design/07-extensibility.md 「3. 테마」 「기본 테마」 행).

import {
  type Binding,
  KeymapFileError,
  type Platform,
  parseKeymapToml,
  readDefaultKeymap,
} from "@pitwall/keymap";

import text from "../../../../extensions/keymap-default/keymaps/default.toml?raw";

/** 번들에 묶은 기본 키맵 파일(레지스트리 응답 전 · 실패 때의 대체값). */
export const DEFAULT_KEYMAP_TOML: string = text;

/** 기본 키맵의 id. extensions/keymap-default/extension.toml 의 `[contributes.keymaps.<id>]`. */
export const DEFAULT_KEYMAP_ID = "pitwall-default";

export interface DefaultKeymap {
  readonly bindings: readonly Binding[];
  /** 어디서 온 키맵인가. 레지스트리 것이면 기여한 확장의 id 도 든다. */
  readonly source:
    | { readonly kind: "bundled" }
    | { readonly kind: "registry"; readonly extension: string };
  /** 레지스트리에서 받은 파일을 쓰지 못한 이유. 있으면 번들 것을 쓴다. */
  readonly error?: string;
}

/**
 * 레지스트리 응답(`registry`, 아직 없으면 `undefined`)에서 기본 키맵을 고른다. 번들 파일은 시험이 검사하므로 읽다 실패하지
 * 않는다 — 실패하면 그대로 던진다.
 */
export function chooseDefaultKeymap(
  registry: { content: string; extension: string } | undefined,
  platform: Platform,
): DefaultKeymap {
  const bundled = (): readonly Binding[] =>
    readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform);
  if (!registry) return { bindings: bundled(), source: { kind: "bundled" } };
  try {
    return {
      bindings: readDefaultKeymap(parseKeymapToml(registry.content), platform),
      source: { kind: "registry", extension: registry.extension },
    };
  } catch (e) {
    const error = e instanceof KeymapFileError ? e.message : String(e);
    return { bindings: bundled(), source: { kind: "bundled" }, error };
  }
}
