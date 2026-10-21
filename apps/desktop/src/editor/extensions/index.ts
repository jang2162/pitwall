// CodeMirror 확장 조립 지점(12-m1-plan M1-09). 에디터 기능 하나 = 이 폴더의 모듈 하나이고, 모듈은 `EditorFeature` 하나를 내보낸다.
// 텍스트 뷰어(../text-viewer.tsx)는 탭마다 `EDITOR_FEATURES` 를 이 순서대로 펼쳐 EditorView 를 만든다.
//
// 기능마다 CodeMirror `Compartment` 하나로 감싼다. 문서 정보(`EditorFeatureContext`)가 바뀌면 — 설정 · 테마(외관) · 언어 수동
// 변경 · 읽기 전용 — 뷰어는 뷰를 다시 만들지 않고 `reconfigureEditor(context)` 로 각 기능의 `extension(context)` 를 다시 불러
// 그 칸만 바꾼다. 그래서 커서 · 스크롤 · undo 기록이 남는다. 기능은 「context 를 받아 확장을 돌려주는 순수 함수」로만 쓰고, 상태를
// 모듈 변수에 두지 않는다.
//
// 기능을 더하는 법:
//   1. 이 폴더에 모듈을 만든다(예: `highlight.ts`). `extension(context)` 은 그 문서(경로 · 판정 · 언어 · 외관 · 읽기 전용)를 보고
//      확장을 돌려준다 — 끌 때는 빈 배열(`[]`)을 돌려준다(예: 판정의 `highlight` 가 거짓이거나 언어가 `null` 이면 강조를 끈다,
//      결정 M1-D9). 바뀌는 값은 그때마다 context 로 받는다 — 다시 구성될 때 다시 불린다.
//   2. 아래 `EDITOR_FEATURES` 에 한 줄로 끼운다. 순서가 우선순위다 — 같은 키 · 같은 facet 은 앞의 것이 이긴다(CodeMirror 의
//      확장 순서 규칙). 키맵을 가진 기능은 `navigation` 보다 앞에 둔다.
//
//      export const highlight: EditorFeature = {
//        id: "highlight",
//        extension: (c) => (c.judgment.highlight && c.language ? [...] : []),
//      };
//
//   - 뷰에서 다시 그 context 가 필요하면(명령 · 상태바) `state.facet(editorContext)` 로 읽는다.
//   - 상태바 같은 바깥에 알릴 것은 context 의 콜백으로 알린다 — 커서는 `cursorReport` 가 이미 `onCursor` 로 보내므로 M1-18 의
//     상태바 항목은 작업 공간의 `onCursor` 를 구독하기만 하면 된다.
//   - 새 값(예: `[editor] font_size`)이 필요하면 `EditorFeatureContext` 에 필드를 하나 더하고, 텍스트 뷰어가 context 를 만드는
//     자리(../text-viewer.tsx `useFeatureContext`)에서 채운다. 그 값이 바뀌면 다시 구성이 저절로 돈다.
//
// 기능끼리는 서로를 import 하지 않는다. 함께 쓰는 값(테마 색 등)은 CSS 변수(src/theme/tokens.ts)나 `context` 로 받는다.

import { Compartment, type Extension, Facet, type TransactionSpec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Judgment } from "@pitwall/rpc-client";

import { base } from "./base";
import { cursorReport } from "./cursor-report";
import { editing } from "./editing";
import { navigation } from "./navigation";
import { readOnly } from "./read-only";
import { theme } from "./theme";

/** 문서의 undo 기록(../live-text.ts). 뷰가 아니라 문서에 있어 탭 사이 · 뷰를 다시 만든 뒤에도 이어진다. */
export interface EditorHistory {
  undo(view: EditorView): boolean;
  redo(view: EditorView): boolean;
}

/** 1부터 센 줄 · 열(열은 글자 단위). */
export interface EditorCursor {
  readonly line: number;
  readonly column: number;
}

/** 확장을 만들 때 기능이 보는 문서 · 탭 정보. 바뀌면 다시 구성된다. */
export interface EditorFeatureContext {
  /** 탭(dockview 패널) id. 한 문서를 여러 탭이 보일 수 있다. */
  readonly panelId: string;
  /** 정규화한 실제 경로(문서 모델의 키). */
  readonly path: string;
  /** `fs.read` 의 판정(인코딩 · 줄바꿈 · 크기 등급 · 강조 여부). */
  readonly judgment: Judgment;
  /** 이 문서를 고칠 수 없다(크기 등급 · 인코딩 · 쓰기 권한). */
  readonly readOnly: boolean;
  /**
   * Linguist 언어 이름. 탭의 수동 변경(`EditorTabParams.language`, M1-18)이 있으면 그것, 없으면 문서를 열 때의 `lang.detect`
   * 결과. `null` 이면 플레인 텍스트.
   */
  readonly language: string | null;
  /** 창의 외관(라이트 · 다크). 색 자체는 CSS 변수로 따라가지만 구문 토큰 테마(M1-17)가 고를 때 쓴다. */
  readonly appearance: "light" | "dark";
  /** 이 뷰의 커서가 바뀌었다(상태바의 줄:열, M1-18). 그 탭이 활성인지는 받는 쪽이 본다. */
  readonly onCursor: (cursor: EditorCursor) => void;
  /** 문서의 undo · redo(`editing`). 고친 내용은 뷰의 트랜잭션으로 문서에 간다 — 기능이 따로 알릴 것은 없다. */
  readonly history: EditorHistory;
}

export interface EditorFeature {
  /** 기능 이름. 시험 · 로그가 쓴다. */
  readonly id: string;
  extension(context: EditorFeatureContext): Extension;
}

/** 뷰의 지금 context. 기능의 명령 · 리스너가 `state.facet(editorContext)` 로 읽는다. */
export const editorContext = Facet.define<EditorFeatureContext, EditorFeatureContext | undefined>({
  combine: (values) => values[0],
});

const contextFeature: EditorFeature = { id: "context", extension: (c) => editorContext.of(c) };

export const EDITOR_FEATURES: readonly EditorFeature[] = [
  contextFeature,
  base,
  theme,
  readOnly,
  cursorReport,
  editing,
  navigation,
];

// 기능마다 칸 하나. 칸은 상태마다 내용을 따로 가지므로 여러 뷰가 같은 칸을 써도 된다.
const compartments = new WeakMap<EditorFeature, Compartment>();

function compartmentOf(feature: EditorFeature): Compartment {
  let c = compartments.get(feature);
  if (!c) {
    c = new Compartment();
    compartments.set(feature, c);
  }
  return c;
}

/** 뷰를 만들 때의 확장: `features` 를 순서대로, 기능마다 칸에 담아 펼친다. */
export function editorExtensions(
  context: EditorFeatureContext,
  features: readonly EditorFeature[] = EDITOR_FEATURES,
): Extension[] {
  return features.map((f) => compartmentOf(f).of(f.extension(context)));
}

/** context 가 바뀐 뒤 `view.dispatch` 에 넘길 트랜잭션: 기능마다 칸을 새 context 로 다시 채운다. 뷰는 그대로다. */
export function reconfigureEditor(
  context: EditorFeatureContext,
  features: readonly EditorFeature[] = EDITOR_FEATURES,
): TransactionSpec {
  return { effects: features.map((f) => compartmentOf(f).reconfigure(f.extension(context))) };
}
