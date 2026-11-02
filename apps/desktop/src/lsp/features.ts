// LSP 기능 표(15-m4-plan M4-07 ③): 에디터 기능 칸 `lsp`(src/editor/extensions/lsp.ts)가 문서가 언어 서버에 열렸을 때
// (`EditorFeatureContext.lsp`) 이 표를 차례로 펼친다. 기능 행(진단 M4-06 · hover · 정의 M4-08 · 아웃라인 M4-10 · 자동완성 ·
// 시그니처 M4-11 · rename · 코드 액션 M4-12 · 포맷 M4-13 · 심볼 M4-16)은 여기에 한 줄씩 끼운다 — `EDITOR_FEATURES` 는 고치지 않는다.
//
// 기능을 더하는 법:
//   1. `src/lsp/` 에 모듈을 만들고 `LspEditorFeature` 하나를 내보낸다. `extension(context, doc)` 은 그 문서(`doc` — 세션 · 요청
//      도우미 · 능력 칸 `doc.capabilities`)를 보고 확장을 돌려준다. 맡을 서버가 없으면(`doc.capabilities.<기능>` 이 없음) 빈 배열을
//      돌려주거나, 능력 칸이 바뀌는 것(`doc.onCapabilities`)을 뷰 플러그인에서 듣는다 — 서버는 문서를 연 뒤에 뜨므로 처음 구성 때는
//      칸이 비어 있는 것이 보통이다.
//   2. 서버에는 자기 Host RPC 로만 닿고, 부를 때는 `doc.request(kind, (at, signal) => …)` 를 거친다(지난 답 · 옛 요청을 버린다).
//   3. 팝업은 `./popup` 의 `openPopup` 으로, 팝업 아래 액션은 `popupActions.of(…)` 를 확장에 넣어 끼운다.
//   4. 아래 표에 한 줄. 순서가 우선순위다(같은 키 · facet 은 앞의 것이 이긴다).
//
// 기능끼리는 서로를 import 하지 않는다(에디터 기능과 같은 규칙 — src/editor/extensions/index.ts).

import type { Extension } from "@codemirror/state";

import type { EditorFeatureContext } from "../editor/extensions";
import type { LspDocument } from "./lsp-sessions";

export interface LspEditorFeature {
  /** 기능 이름(시험 · 로그). */
  readonly id: string;
  extension(context: EditorFeatureContext, doc: LspDocument): Extension;
}

export const LSP_FEATURES: readonly LspEditorFeature[] = [];
