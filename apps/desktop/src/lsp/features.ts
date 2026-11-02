// LSP 기능 표(15-m4-plan M4-07 ③): 에디터 기능 칸 `lsp`(src/editor/extensions/lsp.ts)가 문서가 언어 서버에 열렸을 때
// (`EditorFeatureContext.lsp`) 이 표를 차례로 펼친다. 기능 행(진단 M4-06 · hover · 정의 M4-08 · 아웃라인 M4-10 · 자동완성 ·
// 시그니처 M4-11 · rename · 코드 액션 M4-12 · 포맷 M4-13 · 심볼 M4-16)은 여기에 한 줄씩 끼운다 — `EDITOR_FEATURES` 는 고치지 않는다.
//
// 기능을 더하는 법:
//   1. `src/lsp/` 에 모듈을 만들고 `LspEditorFeature` 하나를 내보낸다. `extension(context, doc)` 은 그 문서(`doc` — 세션 · 요청
//      도우미 · 능력 칸 `doc.capabilities`)를 보고 확장을 돌려준다. 맡을 서버가 없으면(`doc.capabilities.<기능>` 이 없음) 빈 배열을
//      돌려주거나, 능력 칸이 바뀌는 것(`doc.onCapabilities`)을 뷰 플러그인에서 듣는다 — 서버는 문서를 연 뒤에 뜨므로 처음 구성 때는
//      칸이 비어 있는 것이 보통이다.
//      켜고 끄는 기능(거터 전구 등)은 뷰 상태의 `lspCapabilities` 필드를 읽는다 — 칸이 바뀌면 바탕이 `lspCapabilitiesChanged` 를
//      뷰에 보내므로 `StateField` · `ViewPlugin` 의 `update` 에서 그 효과나 필드 값을 보면 된다(플러그인마다 구독하지 않는다).
//   2. 서버에는 자기 Host RPC 로만 닿는다: `doc.call(kind, "<메서드>", (at) => ({ docId: at.docId, version: at.version, … }))` —
//      편집이 Host 에 닿은 뒤 묻고, 같은 `kind` 의 앞 요청을 취소하고, 지난 답(`staleVersion`)을 버린다. 문서 없이 묻는 것
//      (`lsp.diagnostics.list { projectId, worktree, path }`)은 `doc.services.call`, 이벤트(`lsp.diagnostics`)는
//      `doc.services.subscribe(topics, listener)` — 창의 Host 연결 · 프로젝트로 간다. 웹뷰 쪽 파일(lsp-sessions.ts · 창 배선)은
//      고치지 않는다. 답은 늦게 올 수 있다(물러서 있으면 다시 열 때까지 — 최대 30 s): 팝업을 열기 전에 커서 · 마우스가 아직 그
//      자리인지 다시 본다.
//   3. 팝업은 `./popup` 의 `openPopup` 으로, 팝업 아래 액션은 `popupActions.of(…)` 를 확장에 넣어 끼운다.
//   4. 아래 표에 한 줄. 순서가 우선순위다(같은 키 · facet 은 앞의 것이 이긴다).
//
// 기능끼리는 서로를 import 하지 않는다(에디터 기능과 같은 규칙 — src/editor/extensions/index.ts).

import { type Extension, StateEffect, StateField } from "@codemirror/state";

import type { EditorFeatureContext } from "../editor/extensions";
import { editorContext } from "../editor/extensions/context";
import { lspEdit } from "./edit-feature";
import type { LspCapabilities, LspDocument } from "./lsp-sessions";

export interface LspEditorFeature {
  /** 기능 이름(시험 · 로그). */
  readonly id: string;
  extension(context: EditorFeatureContext, doc: LspDocument): Extension;
}

/** 문서의 능력 칸이 바뀌었다(서버가 뜸 · 끝남 · 동적 등록). LSP 기능 칸이 뷰에 보낸다(리뷰 L-2). */
export const lspCapabilitiesChanged = StateEffect.define<LspCapabilities>();

/** 뷰가 아는 지금 능력 칸. 처음은 뷰를 구성할 때의 `doc.capabilities` 다. */
export const lspCapabilities = StateField.define<LspCapabilities>({
  create: (state) => state.facet(editorContext)?.lsp?.capabilities ?? {},
  update(value, tr) {
    for (const e of tr.effects) if (e.is(lspCapabilitiesChanged)) value = e.value;
    return value;
  },
});

export const LSP_FEATURES: readonly LspEditorFeature[] = [
  // rename · 코드 액션 · 빠른 수정(M4-12).
  lspEdit,
];
