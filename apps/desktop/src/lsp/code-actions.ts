// 코드 액션(15-m4-plan M4-12, 10-ui §4.2 「코드」 의 `⌥↩`): 커서 · 선택 범위(와 그 자리의 진단)로 서버에 묻고
// (`lsp.codeAction.list` — 선호 액션 먼저) 팝업 목록(M4-07 팝업 바탕의 `codeAction` 종류)을 띄운다. 고르면 `lsp.codeAction.resolve`
// 로 편집 · 명령을 받아 편집은 적용기(workspace-edit.ts)에, 명령은 `lsp.command.execute`(그 처리 중 서버가 거는 편집도 같은 적용기 —
// 결정 M4-D15)에 넘긴다. 진단 호버의 액션 줄에는 「빠른 수정」 을 끼운다(`quickfix` 만, 그 진단을 문맥으로).
//
// 진단 문맥: 진단 저장은 M4-06 의 것이다. 「빠른 수정」 은 진단 팝업의 `data` 에서 코드 액션을 맡을 서버가 낸 원래 `Diagnostic`
// (`data.lsp` · `data.servers` — `contextDiagnostic`)과 그 버전(`data.version` — 없으면 진단을 싣지 않는다, `null` 은 버전 모름)을
// 꺼내 넘긴다 — Host 는 버전이 다른 진단을 버리고 버전 모름은 그대로 넘긴다. `⌥↩` 은 지금 진단 없이
// 묻는다(나중에 병합하는 쪽이 Host 에서 M4-06 의 범위 조회로 채운다 — 계획 M4-12 행 「구현에서 정한 것」).
//
// 서버: resolve · 명령은 목록을 준 서버(`LspCodeActionList.server`)로만 간다 — 다른 서버가 그 액션의 `data` · 명령을 받지 않게.
//
// 전구: 서버가 코드 액션을 하고 설정 `[lsp] lightbulb` 가 켜져 있으면(기본 켬) 커서가 멈추고 300 ms 뒤 그 자리를 묻고(`automatic`),
// 쓸 수 있는 액션이 있으면 커서 줄의 거터에 전구를 둔다. 누르면 `⌥↩` 과 같다.

import { RangeSet, StateEffect, StateField } from "@codemirror/state";
import {
  EditorView,
  gutter,
  GutterMarker,
  type PluginValue,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type {
  LspCodeActionItem,
  LspCodeActionList,
  LspContextDiagnostics,
} from "@pitwall/rpc-client";

import { editorContext } from "../editor/extensions/context";
import { editWindowOf, type EditWindow } from "./edit-window";
import type { LspDocument } from "./lsp-sessions";
import { closePopup, openPopup } from "./popup";

/** 전구를 묻기 전에 커서가 멈춰 있어야 하는 시간(ms). */
export const LIGHTBULB_DELAY_MS = 300;

export interface CodeActionRequest {
  /** 범위(없으면 선택 범위). */
  from?: number;
  to?: number;
  /** 문맥 진단 — 진단 팝업의 `data`. */
  diagnostic?: unknown;
  only?: string[];
}

/**
 * 진단 팝업의 `data` → 코드 액션을 맡을 서버(`server`)가 낸 원래 진단과 그 버전. M4-06 의 팝업은 `data.lsp` 에 그 자리의 진단 전부
 * (`Diagnostic[]`)를, `data.servers[i]` 에 i 번째 진단을 낸 서버 id 를 싣는다 — 다른 서버의 진단은 넘기지 않는다(그 서버의 `data` ·
 * 코드를 이 서버가 모른다). `data.lsp` 가 진단 하나여도 받는다(서버를 모르면 넘긴다). 버전은 `data.version` — 없으면 싣지 않는다
 * (지금 문서 버전을 찍으면 지난 버전의 진단이 Host 의 버전 검사를 지난다). `null` 은 버전 모름(진단에 버전을 싣지 않는 서버)이라
 * 그대로 싣는다 — Host 가 통과시킨다(M4-06 재리뷰 N1).
 */
export function contextDiagnostic(
  data: unknown,
  server: string | undefined,
): { version: number | null; items: unknown[] } | undefined {
  if (data === null || typeof data !== "object") return undefined;
  const d = data as { lsp?: unknown; servers?: unknown; version?: unknown };
  if (typeof d.version !== "number" && d.version !== null) return undefined;
  const servers = Array.isArray(d.servers) ? (d.servers as unknown[]) : undefined;
  let items: unknown[];
  if (Array.isArray(d.lsp))
    items = d.lsp.filter((_, i) => servers === undefined || servers[i] === server);
  else if (d.lsp !== undefined && d.lsp !== null)
    items = servers === undefined || servers[0] === server ? [d.lsp] : [];
  else return undefined;
  if (items.length === 0) return undefined;
  return { version: d.version, items };
}

function errorText(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 액션 하나를 적용한다(resolve → 편집 → 명령). `server` 는 목록을 준 서버(resolve · 명령이 그 서버로만 간다), `flow` 는 목록을 물을
 * 때 잡은 적용기 기준이다.
 */
export async function runCodeAction(
  doc: LspDocument,
  win: EditWindow,
  item: LspCodeActionItem,
  server: string,
  flow = win.applier.begin(),
): Promise<void> {
  const T = win.strings();
  try {
    const resolved = await doc.request("codeAction.resolve", (at, signal) =>
      doc.services.call(
        "lsp.codeAction.resolve",
        { docId: at.docId, server, action: item.raw as never },
        { signal },
      ),
    );
    if (!resolved) {
      win.notify({ level: "warning", title: T.stale });
      return;
    }
    let edited = false;
    if (resolved.edit) {
      const outcome = await flow.apply(resolved.edit, { label: item.title });
      win.report(outcome, T.codeActionFailed);
      if (outcome.kind !== "applied" && outcome.kind !== "nothing") return;
      edited = outcome.kind === "applied";
    }
    // 편집을 넣었으면 명령의 기준은 그 뒤다(명령을 보내기 직전에 연다). 아니면 목록 때의 기준 그대로.
    if (resolved.command)
      await win.execute(doc, resolved.command, server, edited ? undefined : flow);
  } catch (e) {
    win.notify({ level: "error", title: T.codeActionFailed(errorText(e)) });
  }
}

/** 목록 팝업을 그린다(↑↓ 로 고르고 ↩ 로 적용, `Esc` 는 팝업 바탕이 닫는다). */
function renderList(
  list: LspCodeActionList,
  label: string,
  choose: (item: LspCodeActionItem) => void,
) {
  return (parent: HTMLElement) => {
    const menu = document.createElement("div");
    menu.className = "pw-lsp-code-actions";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", label);
    const buttons: HTMLButtonElement[] = [];
    for (const item of list.actions) {
      const button = document.createElement("button");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      button.className = "pw-lsp-code-action";
      button.dataset["kind"] = item.kind ?? "";
      button.textContent = item.title;
      if (item.isPreferred) button.dataset["preferred"] = "true";
      if (item.disabled) {
        button.disabled = true;
        button.title = item.disabled;
      }
      button.addEventListener("mousedown", (e) => e.preventDefault());
      button.addEventListener("click", () => choose(item));
      buttons.push(button);
      menu.append(button);
    }
    menu.addEventListener("keydown", (e) => {
      const enabled = buttons.filter((b) => !b.disabled);
      const at = enabled.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        enabled[(at + step + enabled.length) % enabled.length]?.focus();
      }
    });
    parent.append(menu);
    queueMicrotask(() => buttons.find((b) => !b.disabled)?.focus());
  };
}

/** 커서 · 선택(또는 `request` 의 범위)의 코드 액션 목록을 띄운다(`⌥↩` · 「빠른 수정」 · 전구). */
export async function showCodeActions(
  view: EditorView,
  win: EditWindow,
  request: CodeActionRequest = {},
): Promise<void> {
  const T = win.strings();
  const doc = view.state.facet(editorContext)?.lsp;
  // 맡을 서버가 없으면(미신뢰 프로젝트 — 결정 M4-D8 · 서버가 뜨는 중) 조용히 아무것도 하지 않는다.
  if (!doc?.capabilities.code_action) return;
  const selection = view.state.selection.main;
  const from = request.from ?? selection.from;
  const to = request.to ?? selection.to;
  const text = view.state.doc;
  const flow = win.applier.begin();
  let list: LspCodeActionList | undefined;
  try {
    list = await doc.call("codeAction", "lsp.codeAction.list", (at) => ({
      docId: at.docId,
      version: at.version,
      from,
      to,
      diagnostics:
        request.diagnostic === undefined
          ? null
          : ((contextDiagnostic(
              request.diagnostic,
              doc.capabilities.code_action?.server,
            ) as LspContextDiagnostics) ?? null),
      only: request.only ?? null,
      trigger: "invoked",
    }));
  } catch (e) {
    win.notify({ level: "error", title: T.codeActionFailed(errorText(e)) });
    return;
  }
  if (!list || !view.state.doc.eq(text)) return;
  if (list.actions.length === 0) {
    win.notify({ level: "info", title: T.codeActionsNone });
    return;
  }
  const answered = list;
  openPopup(view, {
    kind: "codeAction",
    pos: request.from ?? selection.head,
    body: {
      kind: "dom",
      render: renderList(answered, T.codeActions, (item) => {
        closePopup(view, "main");
        view.focus();
        void runCodeAction(doc, win, item, answered.server, flow);
      }),
    },
  });
}

// ---- 전구 ------------------------------------------------------------------------------------------------------------------

const setBulb = StateEffect.define<number | null>();

/** 전구가 선 줄의 시작 위치. 편집이 있으면 걷는다(다시 묻는다). */
export const lightbulbField = StateField.define<number | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setBulb)) return e.value;
    return tr.docChanged ? null : value;
  },
});

class BulbMarker extends GutterMarker {
  override toDOM() {
    const el = document.createElement("span");
    el.className = "pw-lsp-lightbulb";
    el.textContent = "💡";
    el.setAttribute("aria-hidden", "true");
    return el;
  }
}
const bulb = new BulbMarker();

/** 문서 하나의 전구(커서가 멈추면 묻는다). */
export function lightbulb(doc: LspDocument) {
  const asker = ViewPlugin.fromClass(
    class implements PluginValue {
      private timer: ReturnType<typeof setTimeout> | undefined;
      constructor(private readonly view: EditorView) {
        this.schedule();
      }
      update(u: ViewUpdate) {
        if (u.selectionSet || u.docChanged || u.focusChanged) this.schedule();
      }
      private schedule() {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => void this.ask(), LIGHTBULB_DELAY_MS);
      }
      private async ask() {
        const { view } = this;
        const win = editWindowOf(doc.services);
        const show = (line: number | null) => {
          if (view.state.field(lightbulbField) !== line)
            view.dispatch({ effects: setBulb.of(line) });
        };
        if (!win?.lightbulb() || !doc.capabilities.code_action || !view.hasFocus) {
          show(null);
          return;
        }
        const { from, to, head } = view.state.selection.main;
        const text = view.state.doc;
        let list: LspCodeActionList | undefined;
        try {
          list = await doc.call("lightbulb", "lsp.codeAction.list", (at) => ({
            docId: at.docId,
            version: at.version,
            from,
            to,
            diagnostics: null,
            only: null,
            trigger: "automatic",
          }));
        } catch {
          list = undefined;
        }
        // 묻는 사이 커서 · 문서가 바뀌었으면 다음 물음이 정한다.
        if (!view.state.doc.eq(text) || view.state.selection.main.head !== head) return;
        show(list?.actions.some((a) => !a.disabled) ? view.state.doc.lineAt(head).from : null);
      }
      destroy() {
        clearTimeout(this.timer);
      }
    },
  );
  return [
    lightbulbField,
    asker,
    gutter({
      class: "pw-lsp-lightbulb-gutter",
      markers: (view) => {
        const at = view.state.field(lightbulbField);
        return at === null ? RangeSet.empty : RangeSet.of(bulb.range(at));
      },
      domEventHandlers: {
        mousedown(view, line) {
          if (view.state.field(lightbulbField) !== line.from) return false;
          const win = editWindowOf(doc.services);
          if (win) void showCodeActions(view, win);
          return true;
        },
      },
    }),
  ];
}

/** 목록 · 전구의 모양(테마 토큰 `popup.*`). */
export const codeActionTheme = EditorView.baseTheme({
  ".pw-lsp-code-actions": { display: "flex", flexDirection: "column", minWidth: "16em" },
  ".pw-lsp-code-action": {
    textAlign: "left",
    padding: "2px 8px",
    background: "transparent",
    color: "inherit",
    border: "none",
    borderRadius: "2px",
    cursor: "pointer",
  },
  ".pw-lsp-code-action:focus, .pw-lsp-code-action:hover": {
    backgroundColor: "var(--pw-popup-selectionBackground)",
    outline: "none",
  },
  ".pw-lsp-code-action[data-preferred='true']": { fontWeight: "600" },
  ".pw-lsp-code-action:disabled": {
    color: "var(--pw-popup-mutedForeground)",
    cursor: "default",
  },
  ".pw-lsp-lightbulb-gutter .cm-gutterElement": { cursor: "pointer", fontSize: "11px" },
});
