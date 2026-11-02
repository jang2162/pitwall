// 에디터의 진단(15-m4-plan M4-06): Host 가 모은 진단(crates/pw-lsp/src/diagnostics.rs)을 그 문서에 그린다. LSP 기능 표
// (`LSP_FEATURES`, ./features.ts)에 끼운 기능 하나다.
//
// - 받기: 문서를 열면 `lsp.diagnostics.list { projectId, worktree, path }` 로 지금 것을 읽고, 이벤트 `lsp.diagnostics`(경로의 모든
//   서버 진단 — 바뀔 때마다)를 듣는다. 이벤트를 받으면 **곧바로** 장식이 든 트랜잭션을 낸다 — 대기열 · 디바운스가 없다(완료
//   기준 ①). 단, 한글 조합 중이면 조합이 끝날 때까지 쥔다(조합 중의 글자 영역을 다시 그리면 입력기가 끊긴다 — M4-07 의 쥐기와 같은
//   까닭). 목록의 답보다 이벤트가 먼저 오면 목록의 답은 버린다(이벤트가 새것이다).
// - 지난 버전: 진단의 `version` 이 문서의 지금 버전보다 앞서면 그 진단은 그리지 않고(완료 기준 ②), 그 서버가 앞서 그린 버전 있는
//   진단을 편집에 맞춰 옮긴 채 둔다 — 서버가 새 버전의 진단을 보내면 바뀐다. 버전이 없는 진단(열지 않은 파일 · 버전을 알리지 않는
//   서버)은 받은 대로 그린다.
// - 편집으로 밀린 범위는 CodeMirror 매핑(`tr.changes`)으로 옮겨 그린다. 다음 진단이 오기 전까지다.
// - 그리기: 범위에 심각도별 밑줄(오류 · 경고 물결, 정보 점선, 힌트 파선 — 색 `diagnostic.*` 토큰)과 거터 표지(줄마다 가장 심한 것
//   — 오류 ✖ · 경고 ▲ · 정보 ● · 힌트 ◇ 로 모양이 다르다, 화면 읽기 이름은 심각도). 색만으로 가르지 않는다(완료 기준 ⑦). 빈 범위는
//   그 줄의 다음 글자(줄 끝이면 앞 글자)에 긋는다. 쓰이지 않는 코드(`unnecessary`)는 흐리게, 낡은 것(`deprecated`)은 취소선.
// - 호버: 진단 범위 위에 마우스가 머물면(300 ms) 팝업 바탕(./popup.ts)의 `diagnostic` 팝업에 메시지 · 서버 · 출처 · 코드(설명 링크는
//   외부 브라우저 — `http(s)` 만) · 관련 정보를 보인다. 팝업의 `data` 는 `DiagnosticPopupData`(./diagnostics-model.ts — M4-12 의
//   「빠른 수정」 이 `data.lsp` 를 읽는다). 관련 정보의 위치는 글로만 보인다(루트 밖일 수 있어 여는 길은 M4-08 의 토큰 길이다).
// - `F2` · `⇧F2`(problems.tsx 의 액션)는 `gotoDiagnostic` 로 파일 안 다음 · 이전 오류로 커서를 옮기고 그 팝업을 연다 — 마지막에서
//   처음으로 돈다. 오류가 없으면 경고 · 정보 · 힌트를 돈다(IntelliJ 와 같다).

import { type Extension, RangeSet, StateEffect, StateField, type Text } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  type PluginValue,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type { LspDiagnostic, LspDiagnosticsList, LspPosition } from "@pitwall/rpc-client";

import { editorContext } from "../editor/extensions/context";
import {
  callSafely,
  DIAGNOSTICS_TOPIC,
  diagnosticsUi,
  type LspDiagnosticsEvent,
  type LspSeverity,
  normalizeWorktree,
  popupData,
  severityRank,
} from "./diagnostics-model";
import type { LspEditorFeature } from "./features";
import type { LspDocument } from "./lsp-sessions";
import { closePopup, currentPopup, openPopup, type PopupSpec } from "./popup";

/** 호버가 팝업을 열기까지 머무는 시간(CodeMirror `hoverTooltip` 의 기본값과 같다). */
export const HOVER_DELAY = 300;

/** 문서에 놓인 진단 하나. */
export interface PlacedDiagnostic {
  readonly diagnostic: LspDiagnostic;
  readonly from: number;
  readonly to: number;
}

interface DiagnosticsState {
  readonly items: readonly PlacedDiagnostic[];
  readonly decorations: DecorationSet;
  readonly markers: RangeSet<GutterMarker>;
}

/** 이 경로의 진단 전부(Host 이벤트 · 목록의 답)와 그때 문서의 버전. */
export const setDiagnostics = StateEffect.define<{
  readonly diagnostics: readonly LspDiagnostic[];
  readonly version: number;
}>();

/** LSP 위치 → 오프셋(줄 · UTF-16 열 — 줄 밖이면 줄 끝, 문서 밖이면 문서 끝). */
export function offsetOf(doc: Text, p: LspPosition): number {
  if (p.line >= doc.lines) return doc.length;
  const line = doc.line(p.line + 1);
  return Math.min(line.from + p.character, line.to);
}

function place(doc: Text, d: LspDiagnostic): PlacedDiagnostic {
  const from = offsetOf(doc, d.range.start);
  return { diagnostic: d, from, to: Math.max(from, offsetOf(doc, d.range.end)) };
}

const GLYPHS: Record<LspSeverity, string> = {
  error: "✖",
  warning: "▲",
  information: "●",
  hint: "◇",
};

class SeverityMarker extends GutterMarker {
  constructor(
    readonly severity: LspSeverity,
    readonly count: number,
  ) {
    super();
  }
  override eq(other: SeverityMarker) {
    return other.severity === this.severity && other.count === this.count;
  }
  override toDOM() {
    const label = diagnosticsUi.getState().strings.severity[this.severity];
    const span = document.createElement("span");
    span.className = `pw-diagnostic-marker pw-diagnostic-marker-${this.severity}`;
    span.dataset.severity = this.severity;
    span.textContent = GLYPHS[this.severity];
    span.setAttribute("role", "img");
    span.setAttribute("aria-label", label);
    span.title = this.count > 1 ? `${label} (${this.count})` : label;
    return span;
  }
}

/** 거터 너비를 잡는 자리 표지(보이지 않는다) — 화면 읽기 프로그램이 읽지 않게 이름이 없다. */
class SpacerMarker extends GutterMarker {
  override toDOM() {
    const span = document.createElement("span");
    span.className = "pw-diagnostic-marker-spacer";
    span.textContent = GLYPHS.error;
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

function build(doc: Text, items: readonly PlacedDiagnostic[]): DiagnosticsState {
  const marks = [];
  const perLine = new Map<number, { severity: LspSeverity; count: number }>();
  for (const { diagnostic: d, from, to } of items) {
    let start = from;
    let end = to;
    if (start === end) {
      const line = doc.lineAt(start);
      if (end < line.to) end += 1;
      else if (start > line.from) start -= 1;
    }
    if (start < end) {
      const classes = [`pw-diagnostic`, `pw-diagnostic-${d.severity}`];
      if (d.tags.includes("unnecessary")) classes.push("pw-diagnostic-unnecessary");
      if (d.tags.includes("deprecated")) classes.push("pw-diagnostic-deprecated");
      marks.push(
        Decoration.mark({
          class: classes.join(" "),
          attributes: { "data-diagnostic": d.severity },
        }).range(start, end),
      );
    }
    const line = doc.lineAt(from).from;
    const seen = perLine.get(line);
    if (!seen || severityRank(d.severity) < severityRank(seen.severity))
      perLine.set(line, { severity: d.severity, count: (seen?.count ?? 0) + 1 });
    else seen.count += 1;
  }
  const markers = [...perLine.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([at, { severity, count }]) => new SeverityMarker(severity, count).range(at));
  return {
    items,
    decorations: Decoration.set(marks, true),
    markers: RangeSet.of(markers),
  };
}

const EMPTY: DiagnosticsState = {
  items: [],
  decorations: Decoration.none,
  markers: RangeSet.empty,
};

/** 새 진단 목록을 놓는다: 지난 버전의 진단은 버리고, 그런 서버가 앞서 그린 버전 있는 진단은 옮긴 채 둔다(머리 주석). */
function receive(
  doc: Text,
  old: readonly PlacedDiagnostic[],
  incoming: readonly LspDiagnostic[],
  version: number,
): PlacedDiagnostic[] {
  const stale = new Set<string>();
  const fresh: PlacedDiagnostic[] = [];
  for (const d of incoming) {
    if (d.version !== null && d.version < version) stale.add(d.server);
    else fresh.push(place(doc, d));
  }
  const kept = old.filter((p) => stale.has(p.diagnostic.server) && p.diagnostic.version !== null);
  return [...fresh, ...kept].sort(
    (a, b) =>
      a.from - b.from || severityRank(a.diagnostic.severity) - severityRank(b.diagnostic.severity),
  );
}

export const diagnosticsField = StateField.define<DiagnosticsState>({
  create: () => EMPTY,
  update(value, tr) {
    let items = value.items;
    let changed = false;
    if (tr.docChanged && items.length > 0) {
      items = items.map((p) => {
        const from = tr.changes.mapPos(p.from, 1);
        const to = Math.max(from, tr.changes.mapPos(p.to, -1));
        return { ...p, from, to };
      });
      changed = true;
    }
    for (const e of tr.effects)
      if (e.is(setDiagnostics)) {
        items = receive(tr.state.doc, items, e.value.diagnostics, e.value.version);
        changed = true;
      }
    return changed ? build(tr.state.doc, items) : value;
  },
  provide: (f) => [
    EditorView.decorations.from(f, (s) => s.decorations),
    gutter({
      class: "pw-diagnostic-gutter",
      markers: (view) => view.state.field(f).markers,
      initialSpacer: () => new SpacerMarker(),
    }),
  ],
});

/** 뷰에 놓인 진단(시험 · 액션). */
export function diagnosticsOf(view: EditorView): readonly PlacedDiagnostic[] {
  return view.state.field(diagnosticsField, false)?.items ?? [];
}

function servicesOf(view: EditorView) {
  return view.state.facet(editorContext)?.lsp?.services;
}

/** 팝업 본문: 진단마다 심각도 · 메시지 · 서버 · 출처 · 코드(설명 링크) · 관련 정보. 서버의 글은 문자열로만 넣는다. */
function renderDiagnostics(diagnostics: readonly LspDiagnostic[]) {
  return (parent: HTMLElement, view: EditorView) => {
    const T = diagnosticsUi.getState().strings;
    const services = servicesOf(view);
    for (const d of diagnostics) {
      const entry = document.createElement("div");
      entry.className = `pw-diagnostic-entry pw-diagnostic-entry-${d.severity}`;
      entry.dataset.severity = d.severity;
      const head = document.createElement("div");
      head.className = "pw-diagnostic-entry-head";
      const glyph = document.createElement("span");
      glyph.className = `pw-diagnostic-marker pw-diagnostic-marker-${d.severity}`;
      glyph.textContent = GLYPHS[d.severity];
      glyph.setAttribute("aria-hidden", "true");
      const severity = document.createElement("span");
      severity.className = "pw-diagnostic-severity";
      severity.textContent = T.severity[d.severity];
      head.append(glyph, severity);
      const message = document.createElement("div");
      message.className = "pw-diagnostic-message";
      message.textContent = d.message;
      const meta = document.createElement("div");
      meta.className = "pw-diagnostic-meta";
      const code = [d.source, d.code].filter((s): s is string => !!s).join(" ");
      const source = document.createElement("span");
      source.textContent = T.source(d.server, code || null);
      meta.append(source);
      const href = d.codeHref;
      if (href && /^https?:\/\//.test(href) && services?.openExternal) {
        const link = document.createElement("a");
        link.className = "pw-diagnostic-docs";
        link.href = href;
        link.textContent = T.openDocs;
        link.addEventListener("click", (e) => {
          e.preventDefault();
          services.openExternal?.(href);
        });
        meta.append(link);
      }
      entry.append(head, message, meta);
      if (d.related.length > 0) {
        const related = document.createElement("ul");
        related.className = "pw-diagnostic-related";
        related.setAttribute("aria-label", T.related);
        for (const r of d.related) {
          const item = document.createElement("li");
          const where = r.path ?? r.uri;
          item.textContent = `${where}:${r.range.start.line + 1}:${r.range.start.character + 1} ${r.message}`;
          related.append(item);
        }
        entry.append(related);
      }
      parent.append(entry);
    }
  };
}

/** 진단들의 팝업. */
export function diagnosticPopup(items: readonly PlacedDiagnostic[]): PopupSpec {
  const from = Math.min(...items.map((i) => i.from));
  const to = Math.max(...items.map((i) => i.to));
  const diagnostics = items.map((i) => i.diagnostic);
  return {
    kind: "diagnostic",
    pos: from,
    end: to,
    body: { kind: "dom", render: renderDiagnostics(diagnostics) },
    data: popupData(diagnostics),
  };
}

/** 위치 `pos` 에 걸친 진단(끝 자리 포함 — 빈 범위는 그 자리). */
export function diagnosticsAt(view: EditorView, pos: number): PlacedDiagnostic[] {
  return diagnosticsOf(view).filter((p) => p.from <= pos && pos <= Math.max(p.to, p.from + 1));
}

/**
 * 파일 안 다음(`forward`) · 이전 오류로 커서를 옮기고 그 진단 팝업을 연다. 마지막에서 처음으로(처음에서 마지막으로) 돈다. 오류가
 * 없으면 모든 진단을 돈다. 옮길 것이 없으면 `false`.
 */
export function gotoDiagnostic(view: EditorView, forward: boolean): boolean {
  const items = diagnosticsOf(view);
  if (items.length === 0) return false;
  const errors = items.filter((p) => p.diagnostic.severity === "error");
  const pool = [...(errors.length > 0 ? errors : items)].sort((a, b) => a.from - b.from);
  const head = view.state.selection.main.head;
  const target = forward
    ? (pool.find((p) => p.from > head) ?? pool[0]!)
    : ([...pool].reverse().find((p) => p.from < head) ?? pool[pool.length - 1]!);
  view.dispatch({
    selection: { anchor: target.from },
    effects: EditorView.scrollIntoView(target.from, { y: "nearest" }),
    userEvent: "select.diagnostic",
  });
  openPopup(view, diagnosticPopup(pool.filter((p) => p.from === target.from)));
  return true;
}

/** 이벤트 · 목록을 받아 뷰에 놓는 플러그인(머리 주석 「받기」). */
function receiver(doc: LspDocument) {
  return ViewPlugin.define((view): PluginValue => {
    let alive = true;
    let fromEvent = false;
    /** 조합 중에 받은 것(끝나면 놓는다). */
    let held: readonly LspDiagnostic[] | undefined;
    const worktree = normalizeWorktree(doc.worktree);
    const apply = (diagnostics: readonly LspDiagnostic[]) => {
      if (!alive) return;
      if (view.compositionStarted) {
        held = diagnostics;
        return;
      }
      held = undefined;
      view.dispatch({ effects: setDiagnostics.of({ diagnostics, version: doc.version }) });
    };
    const off = doc.services.subscribe([DIAGNOSTICS_TOPIC], (event) => {
      const payload = event.payload as unknown as LspDiagnosticsEvent;
      if (payload.path !== doc.path || normalizeWorktree(payload.worktree) !== worktree) return;
      fromEvent = true;
      apply(payload.diagnostics);
    });
    callSafely(doc.services, "lsp.diagnostics.list", {
      projectId: doc.projectId,
      worktree: doc.worktree ?? null,
      path: doc.path,
      limit: 1000,
    })
      .then((list: LspDiagnosticsList) => {
        // 그사이 이벤트가 왔으면 그것이 새것이다.
        if (fromEvent) return;
        apply(list.files.flatMap((f) => f.diagnostics));
      })
      .catch(() => {
        // 진단이 없는 Host(시험) · 미신뢰 · 끊긴 연결 — 그리지 않을 뿐이다(결정 M4-D8: 조용히 빈다).
      });
    return {
      update(u: ViewUpdate) {
        if (held && !u.view.compositionStarted) {
          const diagnostics = held;
          held = undefined;
          // 업데이트 안에서는 dispatch 할 수 없다.
          queueMicrotask(() => apply(diagnostics));
        }
      },
      destroy() {
        alive = false;
        off();
      },
    };
  });
}

/** 진단 위에 머문 마우스로 진단 팝업을 연다(머리 주석 「호버」). */
const hover = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | undefined;
    pending: string | undefined;
    constructor(readonly view: EditorView) {}
    cancel() {
      clearTimeout(this.timer);
      this.timer = undefined;
      this.pending = undefined;
    }
    move(e: MouseEvent) {
      const pos = this.view.posAtCoords({ x: e.clientX, y: e.clientY });
      const items = pos === null ? [] : diagnosticsAt(this.view, pos);
      if (items.length === 0) {
        this.cancel();
        return;
      }
      const key = items.map((i) => `${i.from}:${i.to}:${i.diagnostic.message}`).join("|");
      const open = currentPopup(this.view, "main");
      if (open?.kind === "diagnostic" && this.pending === undefined && open.pos === items[0]!.from)
        return;
      if (this.pending === key) return;
      this.cancel();
      this.pending = key;
      this.timer = setTimeout(() => {
        this.timer = undefined;
        this.pending = undefined;
        // 그사이 편집 · 다시 받기로 바뀌었으면 지금 것으로.
        const now = diagnosticsAt(this.view, pos!);
        if (now.length > 0) openPopup(this.view, diagnosticPopup(now));
      }, HOVER_DELAY);
    }
    leave(e: MouseEvent) {
      this.cancel();
      // 팝업으로 옮겨 가는 마우스는 팝업을 닫지 않는다.
      const to = e.relatedTarget as Element | null;
      if (to?.closest?.(".pw-lsp-popup")) return;
      if (currentPopup(this.view, "main")?.kind === "diagnostic") closePopup(this.view, "main");
    }
    destroy() {
      this.cancel();
    }
  },
  {
    eventHandlers: {
      mousemove(e) {
        this.move(e);
      },
      mouseleave(e) {
        this.leave(e);
      },
    },
  },
);

const theme = EditorView.baseTheme({
  ".pw-diagnostic": {
    textDecorationLine: "underline",
    textDecorationThickness: "1px",
    textUnderlineOffset: "3px",
    textDecorationSkipInk: "none",
  },
  ".pw-diagnostic-error": {
    textDecorationStyle: "wavy",
    textDecorationColor: "var(--pw-diagnostic-error)",
  },
  ".pw-diagnostic-warning": {
    textDecorationStyle: "wavy",
    textDecorationColor: "var(--pw-diagnostic-warning)",
  },
  ".pw-diagnostic-information": {
    textDecorationStyle: "dotted",
    textDecorationThickness: "2px",
    textDecorationColor: "var(--pw-diagnostic-info)",
  },
  ".pw-diagnostic-hint": {
    textDecorationStyle: "dashed",
    textDecorationColor: "var(--pw-diagnostic-hint)",
  },
  ".pw-diagnostic-unnecessary": { opacity: "0.6" },
  ".pw-diagnostic-deprecated": { textDecorationLine: "underline line-through" },
  ".pw-diagnostic-gutter .cm-gutterElement": {
    padding: "0 2px",
    textAlign: "center",
    fontSize: "10px",
    lineHeight: "inherit",
  },
  ".pw-diagnostic-marker-error": { color: "var(--pw-diagnostic-error)" },
  ".pw-diagnostic-marker-warning": { color: "var(--pw-diagnostic-warning)" },
  ".pw-diagnostic-marker-information": { color: "var(--pw-diagnostic-info)" },
  ".pw-diagnostic-marker-hint": { color: "var(--pw-diagnostic-hint)" },
  ".pw-diagnostic-entry + .pw-diagnostic-entry": {
    borderTop: "1px solid var(--pw-popup-border)",
    marginTop: "6px",
    paddingTop: "6px",
  },
  ".pw-diagnostic-entry-head": { display: "flex", gap: "6px", fontWeight: "600" },
  ".pw-diagnostic-message": { whiteSpace: "pre-wrap", marginTop: "2px" },
  ".pw-diagnostic-meta": {
    display: "flex",
    gap: "8px",
    marginTop: "4px",
    color: "var(--pw-popup-mutedForeground)",
  },
  ".pw-diagnostic-related": {
    margin: "4px 0 0",
    paddingLeft: "16px",
    color: "var(--pw-popup-mutedForeground)",
  },
});

/** LSP 기능 표의 진단(./features.ts 의 `LSP_FEATURES` 한 줄). */
export const diagnosticsFeature: LspEditorFeature = {
  id: "diagnostics",
  extension: (_context, doc): Extension => [diagnosticsField, receiver(doc), hover, theme],
};
