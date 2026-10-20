// CodeMirror 확장 조립 지점(12-m1-plan M1-09). 에디터 기능 하나 = 이 폴더의 모듈 하나이고, 모듈은 `EditorFeature` 하나를 내보낸다.
// 텍스트 뷰어(../text-viewer.tsx)는 탭마다 `EDITOR_FEATURES` 를 이 순서대로 펼쳐 EditorView 를 만든다.
//
// 기능을 더하는 법:
//   1. 이 폴더에 모듈을 만든다(예: `highlight.ts`). `extension(context)` 은 그 문서(경로 · 판정 · 읽기 전용)를 보고 확장을 돌려준다
//      — 끌 때는 빈 배열(`[]`)을 돌려준다(예: 판정의 `highlight` 가 거짓이면 강조를 끈다, 결정 M1-D9).
//   2. 아래 `EDITOR_FEATURES` 에 한 줄로 끼운다. 순서가 우선순위다 — 같은 키 · 같은 facet 은 앞의 것이 이긴다(CodeMirror 의
//      확장 순서 규칙). 키맵을 가진 기능은 `navigation` 보다 앞에 둔다.
//
//      export const highlight: EditorFeature = { id: "highlight", extension: (c) => (c.judgment.highlight ? [...] : []) };
//
// 기능끼리는 서로를 import 하지 않는다. 함께 쓰는 값(테마 색 등)은 CSS 변수(src/theme/tokens.ts)나 이 파일의 `context` 로 받는다.

import type { Extension } from "@codemirror/state";
import type { Judgment } from "@pitwall/rpc-client";

import { base } from "./base";
import { navigation } from "./navigation";
import { readOnly } from "./read-only";
import { theme } from "./theme";

/** 확장을 만들 때 기능이 보는 문서 정보. */
export interface EditorFeatureContext {
  /** 정규화한 실제 경로(문서 모델의 키). */
  readonly path: string;
  /** `fs.read` 의 판정(인코딩 · 줄바꿈 · 크기 등급 · 강조 여부). */
  readonly judgment: Judgment;
  /** 이 문서를 고칠 수 없다(크기 등급 · 인코딩 · 쓰기 권한). */
  readonly readOnly: boolean;
}

export interface EditorFeature {
  /** 기능 이름. 시험 · 로그가 쓴다. */
  readonly id: string;
  extension(context: EditorFeatureContext): Extension;
}

export const EDITOR_FEATURES: readonly EditorFeature[] = [base, theme, readOnly, navigation];

/** `features` 를 순서대로 펼친 확장. */
export function editorExtensions(
  context: EditorFeatureContext,
  features: readonly EditorFeature[] = EDITOR_FEATURES,
): Extension[] {
  return features.map((f) => f.extension(context));
}
