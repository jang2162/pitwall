// rename(15-m4-plan M4-12, 10-ui §4.2 「코드」 의 `⇧F6` — 에디터 포커스. 파일 트리 포커스의 `⇧F6` 은 파일 이름 바꾸기다):
// 커서 자리를 서버에 물어(`lsp.rename.prepare` — 범위 · 처음 이름, 서버가 범위를 정하지 않으면 커서의 낱말) 그 자리에 인라인 입력을
// 띄우고, ↩ 로 정한 이름의 편집(`lsp.rename`)을 적용기(workspace-edit.ts)에 넘긴다 — 여러 파일이면 미리보기를 거쳐 디스크에 쓰고
// 「되돌리기」 알림(결정 M4-D14).
//
// 입력: 이름 범위 아래에 붙는 CodeMirror 툴팁(`view.dom` 안). ↩ 는 한글 조합 중이 아닐 때만 정한다. `Esc` · 포커스를 잃음 · 문서
// 편집은 취소다 — `Esc` 는 `preventDefault` 해 창 키맵의 「커서 하나로」(M4-14)가 그 눌림에서 돌지 않게 한다.

import { StateEffect, StateField } from "@codemirror/state";
import { EditorView, showTooltip, type Tooltip } from "@codemirror/view";

import { editorContext } from "../editor/extensions/context";
import type { EditWindow } from "./edit-window";

interface RenameInput {
  readonly from: number;
  readonly to: number;
  readonly value: string;
  /** 입력 칸의 이름 · 아래 안내(창의 언어). */
  readonly label: string;
  readonly hint: string;
  readonly done: (name: string | null) => void;
}

const setRenameInput = StateEffect.define<RenameInput | null>();

/** 떠 있는 rename 입력. 문서가 바뀌면 취소한다. */
export const renameInputField = StateField.define<RenameInput | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setRenameInput)) return e.value;
    if (value && tr.docChanged) {
      queueMicrotask(() => value.done(null));
      return null;
    }
    return value;
  },
  provide: (f) =>
    showTooltip.from(f, (input): Tooltip | null =>
      input
        ? {
            pos: input.from,
            end: input.to,
            above: false,
            create: (view) => renameDom(view, input),
          }
        : null,
    ),
});

function renameDom(view: EditorView, input: RenameInput) {
  const dom = document.createElement("div");
  dom.className = "cm-lsp-rename";
  const field = document.createElement("input");
  field.type = "text";
  field.value = input.value;
  field.spellcheck = false;
  field.setAttribute("aria-label", input.label);
  field.className = "cm-lsp-rename-input";
  const hint = document.createElement("div");
  hint.className = "cm-lsp-rename-hint";
  hint.textContent = input.hint;
  dom.append(field, hint);
  let settled = false;
  const finish = (name: string | null) => {
    if (settled) return;
    settled = true;
    view.dispatch({ effects: setRenameInput.of(null) });
    view.focus();
    input.done(name);
  };
  field.addEventListener("keydown", (event) => {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter") {
      event.preventDefault();
      finish(field.value);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      finish(null);
    }
  });
  field.addEventListener("blur", () => {
    // 툴팁을 다시 그리느라 잠깐 잃는 포커스는 지나간다.
    setTimeout(() => {
      if (!settled && document.activeElement !== field) finish(null);
    }, 0);
  });
  return {
    dom,
    mount() {
      field.focus();
      field.select();
    },
    destroy() {
      if (!settled) {
        settled = true;
        input.done(null);
      }
    },
  };
}

/** `from` ~ `to` 에 이름 입력을 띄우고 정한 이름을 기다린다(취소면 `null`). */
export function askName(
  view: EditorView,
  input: Omit<RenameInput, "done">,
): Promise<string | null> {
  return new Promise<string | null>((done) =>
    view.dispatch({ effects: setRenameInput.of({ ...input, done }) }),
  );
}

/** 뷰의 커서 자리를 rename 한다(`⇧F6`). */
export async function renameAt(view: EditorView, win: EditWindow): Promise<void> {
  const T = win.strings();
  const doc = view.state.facet(editorContext)?.lsp;
  // 맡을 서버가 없으면(미신뢰 프로젝트 — 결정 M4-D8 · 서버가 뜨는 중 · rename 을 하지 않는 서버) 조용히 아무것도 하지 않는다.
  if (!doc?.capabilities.rename) return;
  const head = view.state.selection.main.head;
  const text = view.state.doc;
  const prepared = await doc.call("rename", "lsp.rename.prepare", (at) => ({
    docId: at.docId,
    version: at.version,
    offset: head,
  }));
  // 묻는 사이 문서 · 커서가 바뀌었으면 그만둔다(답은 그 전 텍스트 기준이다).
  if (!prepared || !view.state.doc.eq(text) || view.state.selection.main.head !== head) return;
  let from: number;
  let to: number;
  let value: string;
  const target = prepared.target;
  if (target.kind === "invalid") {
    win.notify({ level: "info", title: T.renameInvalid(target.message) });
    return;
  }
  if (target.kind === "range") {
    from = target.from;
    to = target.to;
    value = target.placeholder ?? text.sliceString(from, to);
  } else {
    const word = view.state.wordAt(head);
    if (!word) {
      win.notify({ level: "info", title: T.renameInvalid(null) });
      return;
    }
    from = word.from;
    to = word.to;
    value = text.sliceString(from, to);
  }
  const name = await askName(view, {
    from,
    to,
    value,
    label: T.renameInput,
    hint: T.renameHint,
  });
  if (name === null || name === "" || name === value) return;
  // 적용기의 기준(요청 뒤의 `fs.changed` · 동기화한 문서의 버전)은 요청을 보내기 전에 잡는다.
  const flow = win.applier.begin();
  let answer;
  try {
    answer = await doc.call("rename", "lsp.rename", (at) => ({
      docId: at.docId,
      version: at.version,
      offset: from,
      newName: name,
    }));
  } catch (e) {
    win.notify({
      level: "error",
      title: T.renameFailed(e instanceof Error ? e.message : String(e)),
    });
    return;
  }
  if (!answer) {
    win.notify({ level: "warning", title: T.stale });
    return;
  }
  win.report(await flow.apply(answer.edit, { label: name }), T.renameFailed);
}

/** rename 입력의 모양(테마 토큰 `popup.*` — M4-07). */
export const renameTheme = EditorView.baseTheme({
  ".cm-tooltip.cm-lsp-rename, .cm-lsp-rename": {
    padding: "4px",
    backgroundColor: "var(--pw-popup-background)",
    color: "var(--pw-popup-foreground)",
    border: "1px solid var(--pw-popup-border)",
    borderRadius: "4px",
  },
  ".cm-lsp-rename-input": {
    font: "inherit",
    minWidth: "12em",
    padding: "1px 4px",
    backgroundColor: "var(--pw-editor-background)",
    color: "var(--pw-editor-foreground)",
    border: "1px solid var(--pw-ui-accent)",
    borderRadius: "2px",
    outline: "none",
  },
  ".cm-lsp-rename-hint": {
    fontSize: "11px",
    color: "var(--pw-popup-mutedForeground)",
    marginTop: "2px",
  },
});
