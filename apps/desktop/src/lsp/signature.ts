// 시그니처 도움말(15-m4-plan M4-11, 10-ui §4.2 「코드 (LSP · 편집)」 의 `⌘P`): 서버의 시그니처 트리거 문자(`(` 등)를 치거나 `⌘P`
// (`editor.parameterInfo`)로 부르면 `lsp.signatureHelp` 를 묻고, 답을 팝업 바탕(./popup.ts)의 「편집해도 유지」 팝업
// (`kind: "signature"`, `keepOnEdit`)으로 보인다 — 인자를 치는 동안 자리를 따라가고 호버 · 진단이 떠도 남는다.
//
// - 열려 있는 동안: 글을 고치거나 커서가 움직이면 다시 묻는다(`,` 같은 다시 트리거 문자는 `character`, 나머지는 `contentChange`) —
//   지금 인자 강조가 `,` 마다 옮겨진다. 서버가 도움말이 없다고 하면(커서가 호출 밖) 닫는다. 다시 물을 때는 보이는 도움말(`raw`)을
//   함께 보내 서버가 고른 시그니처를 이어 간다.
// - 여러 시그니처: `↑` `↓` 로 고른다(팝업이 열려 있고 자동완성 목록이 닫혀 있을 때만 — 아니면 커서 이동이다).
// - 요청은 LSP 요청 도우미(`doc.call` — 편집이 Host 에 닿은 뒤 묻고, 같은 종류 앞 요청을 취소하고, 지난 답을 버린다)로 한다. 트랜잭션
//   안에서 부르면 문서 동기화가 아직 그 편집을 받지 않았을 수 있어 한 마이크로태스크 뒤에 묻는다(대기열 · 디바운스는 없다).
// - 한글 조합 중에는 묻지 않는다(조합 중의 바뀜은 서버에 가지 않는다 — M4-07 ⓔ). 미신뢰 · 서버 없음이면 능력 칸이 비어 조용히 아무것도
//   하지 않는다. 칸이 빈 채 친 글자는 칸을 묻는 중이면 그 답을 기다려 판정한다(`askWhenSettled` — 자동완성과 같다, ./completion.ts).
// - 서버가 준 글(시그니처 · 문서)은 글자(`textContent`)나 HTML 을 끈 Markdown(`renderPopupBody`)으로만 넣는다.

import type { Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import type { LspMarkup, LspSignatureHelp, LspSignatureTriggerKind } from "@pitwall/rpc-client";

import type { LspDocument } from "./lsp-sessions";
import { closePopup, currentPopup, openPopup, renderPopupBody } from "./popup";

/** 서버의 시그니처 도움말 옵션(`signatureHelpProvider` · 동적 등록). */
interface SignatureOptions {
  triggerCharacters?: string[];
  retriggerCharacters?: string[];
}

function signatureOptions(doc: LspDocument): SignatureOptions | undefined {
  const cap = doc.capabilities.signature_help;
  if (!cap) return undefined;
  return typeof cap.options === "object" && cap.options !== null && !Array.isArray(cap.options)
    ? (cap.options as SignatureOptions)
    : {};
}

interface Ask {
  readonly kind: LspSignatureTriggerKind;
  readonly character?: string;
}

/** 뷰마다 시그니처 도움말을 맡는 것. `⌘P` 액션 · 자동완성의 명령이 찾아 부른다. */
class SignatureHelper {
  /** 보이는 도움말과 고른 시그니처. */
  help: LspSignatureHelp | null = null;
  selected = 0;
  /** 다음 마이크로태스크에 물을 것. 한 틱에 여럿이면 하나로 — `contentChange` 보다 트리거 · 부름이 이긴다. */
  private pending: Ask | undefined;
  private destroyed = false;

  constructor(
    readonly view: EditorView,
    readonly doc: LspDocument,
  ) {}

  get open() {
    return currentPopup(this.view, "signature") !== null;
  }

  /** 다음 마이크로태스크에 묻는다(트랜잭션 안에서 불릴 수 있다). */
  ask(what: Ask) {
    const queued = this.pending !== undefined;
    if (!queued || what.kind !== "contentChange") this.pending = what;
    if (queued) return;
    queueMicrotask(() => {
      const next = this.pending;
      this.pending = undefined;
      if (next) void this.request(next);
    });
  }

  /**
   * 능력 칸이 빈 채 친 글자 `char`: 칸을 묻는 중이면(서버가 막 준비돼 `lsp.status` 로 다시 묻는 틈) 그 답을 기다려 트리거 문자면
   * 묻는다. 그사이 글이 바뀌었으면 묻지 않는다 — 그 바뀜이 스스로 판정된다. 묻는 중이 아니면 곧바로 풀려 「서버 없음」 그대로다.
   */
  askWhenSettled(char: string) {
    const text = this.view.state.doc;
    void this.doc.capabilitiesSettled().then(() => {
      if (this.destroyed || this.view.state.doc !== text) return;
      if (signatureOptions(this.doc)?.triggerCharacters?.includes(char))
        this.ask({ kind: "character", character: char });
    });
  }

  private async request(what: Ask) {
    const { view, doc } = this;
    if (this.destroyed || view.compositionStarted || !signatureOptions(doc)) return;
    const head = view.state.selection.main.head;
    const wasOpen = this.open;
    const active = wasOpen ? this.help?.raw : undefined;
    let answer;
    try {
      answer = await doc.call("signature", "lsp.signatureHelp", (at) => ({
        docId: at.docId,
        version: at.version,
        offset: head,
        triggerKind: what.kind,
        ...(what.character !== undefined && { triggerCharacter: what.character }),
        isRetrigger: wasOpen,
        ...(active !== undefined && { active }),
      }));
    } catch (e) {
      console.warn("[lsp] signature help failed", e);
      return;
    }
    if (answer === undefined || this.destroyed) return;
    // 답이 오는 사이 커서가 옮겨졌으면 그 자리의 새 요청이 맡는다.
    if (view.state.selection.main.head !== head) return;
    if (!answer.help) {
      this.close();
      return;
    }
    const keep = this.help && this.open ? this.selected : answer.help.activeSignature;
    this.help = answer.help;
    this.selected = Math.min(keep, answer.help.signatures.length - 1);
    this.show(wasOpen ? (currentPopup(view, "signature")?.pos ?? head) : head);
  }

  close() {
    this.help = null;
    if (this.open) closePopup(this.view, "signature");
  }

  /** 여러 시그니처 사이를 옮긴다. 옮겼으면 참. */
  cycle(step: number): boolean {
    const help = this.help;
    if (!help || help.signatures.length < 2 || !this.open) return false;
    const n = help.signatures.length;
    this.selected = (this.selected + step + n) % n;
    this.show(currentPopup(this.view, "signature")?.pos ?? this.view.state.selection.main.head);
    return true;
  }

  private show(pos: number) {
    const help = this.help;
    if (!help) return;
    const index = this.selected;
    const signature = help.signatures[index]!;
    const count = help.signatures.length;
    openPopup(this.view, {
      kind: "signature",
      pos,
      above: true,
      keepOnEdit: true,
      data: { signature: index, activeParameter: signature.activeParameter },
      body: {
        kind: "dom",
        render: (parent, view) => renderSignature(parent, view, signature, index, count),
      },
    });
  }

  destroy() {
    this.destroyed = true;
  }
}

function markupBody(markup: LspMarkup) {
  return markup.markdown
    ? ({ kind: "markdown", markdown: markup.value } as const)
    : ({ kind: "text", text: markup.value } as const);
}

/** 시그니처 하나를 그린다: 글(지금 인자는 굵게), 여러 개면 `n/m`, 지금 인자 · 시그니처의 문서. */
function renderSignature(
  parent: HTMLElement,
  view: EditorView,
  signature: LspSignatureHelp["signatures"][number],
  index: number,
  count: number,
) {
  const line = document.createElement("div");
  line.className = "pw-lsp-signature";
  if (count > 1) {
    const at = document.createElement("span");
    at.className = "pw-lsp-signature-count";
    at.textContent = `${index + 1}/${count}`;
    line.append(at, " ");
  }
  const label = document.createElement("code");
  const active =
    signature.activeParameter === null
      ? undefined
      : signature.parameters[signature.activeParameter];
  if (active && active.to > active.from) {
    const strong = document.createElement("span");
    strong.className = "pw-lsp-signature-active";
    strong.textContent = signature.label.slice(active.from, active.to);
    label.append(signature.label.slice(0, active.from), strong, signature.label.slice(active.to));
  } else label.textContent = signature.label;
  line.append(label);
  parent.append(line);
  for (const doc of [active?.documentation, signature.documentation]) {
    if (!doc) continue;
    const part = document.createElement("div");
    part.className = "pw-lsp-signature-doc";
    renderPopupBody(part, markupBody(doc), view);
    parent.append(part);
  }
}

const helpers = new WeakMap<EditorView, SignatureHelper>();

/** `⌘P`: 시그니처 도움말을 부른다. 이 뷰에 시그니처를 맡을 서버가 없으면 거짓. */
export function triggerSignatureHelp(view: EditorView): boolean {
  const helper = helpers.get(view);
  if (!helper || !signatureOptions(helper.doc)) return false;
  helper.ask({ kind: "invoked" });
  return true;
}

/** 열린 시그니처 도움말의 시그니처를 옮긴다(`↑` `↓`). */
export function cycleSignature(view: EditorView, step: number): boolean {
  return helpers.get(view)?.cycle(step) ?? false;
}

/** 이 트랜잭션들에서 친 마지막 글자(사람이 친 입력만 — 조합 · 붙여넣기 · 완성은 아니다). */
function typed(update: ViewUpdate): string | undefined {
  let char: string | undefined;
  for (const tr of update.transactions) {
    if (!tr.docChanged || !tr.isUserEvent("input.type") || tr.isUserEvent("input.type.compose"))
      continue;
    tr.changes.iterChanges((_fa, _ta, _fb, _tb, inserted) => {
      const text = inserted.toString();
      if (text) char = text.slice(-1);
    });
  }
  return char;
}

const signaturePlugin = (doc: LspDocument) =>
  ViewPlugin.define((view) => {
    const helper = new SignatureHelper(view, doc);
    helpers.set(view, helper);
    return {
      update(u: ViewUpdate) {
        if (!u.docChanged && !u.selectionSet) return;
        if (u.view.compositionStarted) return;
        const options = signatureOptions(doc);
        const char = typed(u);
        if (!options) {
          if (char !== undefined) helper.askWhenSettled(char);
          return;
        }
        const open = helper.open;
        if (!open && helper.help) helper.help = null;
        if (char !== undefined && options.triggerCharacters?.includes(char))
          helper.ask({ kind: "character", character: char });
        else if (open && char !== undefined && options.retriggerCharacters?.includes(char))
          helper.ask({ kind: "character", character: char });
        else if (open) helper.ask({ kind: "contentChange" });
      },
      destroy() {
        helper.destroy();
        if (helpers.get(view) === helper) helpers.delete(view);
      },
    };
  });

const signatureTheme = EditorView.baseTheme({
  ".pw-lsp-signature code": { fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)" },
  ".pw-lsp-signature-active": { fontWeight: "700", textDecoration: "underline" },
  ".pw-lsp-signature-count": { color: "var(--pw-popup-mutedForeground)" },
  ".pw-lsp-signature-doc": {
    marginTop: "4px",
    paddingTop: "4px",
    borderTop: "1px solid var(--pw-popup-border)",
  },
});

/** 시그니처 도움말의 확장. 자동완성 기능(./completion.ts)이 함께 펼친다. */
export function signatureHelp(doc: LspDocument): Extension {
  return [signaturePlugin(doc), signatureTheme];
}
