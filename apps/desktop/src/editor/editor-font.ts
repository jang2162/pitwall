// 코드 글꼴 설정(결정 M1-D17): 전역 `settings.toml` 의 `[editor] font_family` · `font_size`. 설정이 다시 읽히면(`config.changed`)
// `config.get` 이 다시 불리고(rpc/host.tsx) 열린 에디터가 재시작 없이 바뀐다 — 텍스트 뷰어가 이 값을 기능 context 에 넣어 같은
// 뷰를 다시 구성한다(extensions/operations.ts). 키의 뜻은 docs/design/03-architecture.md 「`[editor]`」.
//
// 틀린 값(문자열이 아닌 글꼴 · 빈 글꼴 · 범위 밖 크기)은 없는 것으로 보고 기본값을 쓴다 — 설정 하나가 틀렸다고 에디터가 그려지지
// 않으면 안 된다.

import type { ConfigView } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { useRpcQuery } from "../rpc/host";
import type { EditorFont } from "./extensions";

/** 받는 글꼴 크기(px). 이 밖은 기본값(13px)을 쓴다. */
export const FONT_SIZE_RANGE = { min: 6, max: 72 } as const;

/** 설정 스냅숏에서 글꼴을 읽는다. */
export function editorFontOf(config: ConfigView | undefined): EditorFont {
  const settings = config?.snapshot.files.find((f) => f.id === "settings")?.value as
    | { editor?: { font_family?: unknown; font_size?: unknown } }
    | undefined;
  const editor = settings?.editor;
  const family =
    typeof editor?.font_family === "string" && editor.font_family.trim() !== ""
      ? editor.font_family
      : undefined;
  const size =
    typeof editor?.font_size === "number" &&
    editor.font_size >= FONT_SIZE_RANGE.min &&
    editor.font_size <= FONT_SIZE_RANGE.max
      ? editor.font_size
      : undefined;
  return { family, size };
}

/** 지금 설정의 코드 글꼴. 값이 같으면 같은 객체다. */
export function useEditorFont(): EditorFont {
  const config = useRpcQuery("config.get", {});
  const { family, size } = editorFontOf(config.data);
  return useMemo(() => ({ family, size }), [family, size]);
}
