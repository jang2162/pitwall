// 내비게이션(15-m4-plan M4-08): hover · 빠른 문서 · ⌘+호버 선언 미리보기 · 선언 / 구현으로 이동 · 사용처와 그 위치 팝업.
// LSP 기능 표(`LSP_FEATURES`, ./features.ts)의 한 줄이고, 루트 밖 문서(외부 소스 탭 — ./outside-sources.tsx)도 같은 확장을 쓴다
// — 둘의 차이는 요청을 보내는 길(`NavSource`)뿐이다: 열린 문서는 요청 도우미(`LspDocument.call` — 버전 · 취소 · 지난 답 버림 ·
// 한글 조합 중에는 묻지 않음, M4-07), 외부 소스는 그 토큰으로 같은 서버에(`lsp.hover { token }` 등 — Host 가 토큰을 준 인스턴스에
// 묻는다, 결정 M4-D13).
//
// - 호버(10-ui §5.1): 마우스가 한 낱말 위에 0.5 s 머물면 hover 를 묻고 팝업(`kind: "hover"`)을 띄운다. 답이 오면 마우스가 아직 그
//   자리인지 다시 본다. 마우스가 그 낱말과 팝업을 벗어나면 닫는다. `⌘`(Win/Linux `Ctrl`)를 누른 채면 낱말에 밑줄을 긋고, 머물면
//   hover 대신 선언 미리보기(정의의 앞 몇 줄을 읽기 전용 코드 블록으로)를 띄운다.
// - 빠른 문서(`F1` · Win/Linux `Ctrl+Q`): 커서 자리의 hover 를 고정 팝업으로 — 마우스가 움직여도 닫히지 않고 본문은 팝업 안에서
//   스크롤된다. 닫힘은 팝업 바탕의 규칙(편집 · 에디터 스크롤 · `Esc`).
// - 선언으로(`⌘B` · `⌘+클릭`): 결과 하나면 바로, 여럿이면 위치 팝업, 커서가 선언 위면(결과가 커서 자리를 품으면) 사용처(10-ui §5.3).
//   구현으로(`⌘⌥B` · `⌘⌥+클릭`)는 하나면 바로, 여럿이면 팝업. 사용처(`⌥F7`)는 늘 팝업이고 `⌘↩`(Win/Linux `Ctrl+Enter`)로
//   「검색 결과」 툴 윈도우에 고정한다(결정 M4-D22).
// - 위치는 모두 위치로 가기(`workspace.navigateTo` — M4-09)로 연다: 떠나는 자리가 뒤로 가기 기록에 든다. 루트 밖 위치는 토큰째
//   넘기므로 작업 공간이 외부 소스 탭(`OutsideSources`)으로 연다.
// - 서버가 준 글(hover · 미리보기 줄)은 믿지 않는다: hover 의 Markdown 은 팝업 바탕의 HTML 을 끈 렌더(이미지는 주소를 읽지 않고
//   링크는 http(s) · mailto 만 외부 브라우저로), 위치 팝업의 줄 글은 `textContent` 로만 넣는다.
// - 맡을 서버가 없으면(능력 칸에 그 기능이 없음 — 미신뢰 프로젝트 · 서버 없음 · 아직 뜨는 중) 묻지 않고 조용히 아무것도 하지 않는다.
//
// 창의 것(작업 공간 · 문자열 · 고정 · 플랫폼)은 창이 `setNavigationHost` 로 넘긴다(./navigation-host.tsx).

import { Facet, RangeSetBuilder, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  type PluginValue,
  ViewPlugin,
} from "@codemirror/view";
import type {
  LspHoverResult,
  LspLocation,
  LspLocationsResult,
  LspPositionParams,
  LspReferencesParams,
} from "@pitwall/rpc-client";

import type { NavTarget } from "../editor/navigation";
import type { LspEditorFeature } from "./features";
import type { LspDocument, LspServices } from "./lsp-sessions";
import { closePopup, currentPopup, openPopup, type PopupSpec } from "./popup";

/** 호버가 팝업을 띄우기까지(10-ui §5.1). */
export const HOVER_DELAY_MS = 500;
/** 선언 미리보기의 줄 수. */
export const PREVIEW_LINES = 8;

/** 위치 목록 요청의 종류(Host RPC). */
export type LocationMethod =
  | "lsp.definition"
  | "lsp.declaration"
  | "lsp.typeDefinition"
  | "lsp.implementation"
  | "lsp.references";

/** 요청을 보내는 길: 열린 문서(요청 도우미) 또는 외부 소스(토큰). */
export interface NavSource {
  /** Linguist 언어 이름(미리보기 코드 블록의 색). */
  readonly language: string | null;
  readonly services: LspServices;
  /** 이 문서의 자리(「선언 위」 판정 · 위치를 열 때의 워크트리). */
  readonly path: string;
  readonly worktree?: string;
  readonly token?: string;
  /** 그 기능을 맡을 서버가 있나(없으면 묻지 않는다). */
  offers(feature: "hover" | "definition" | "implementation" | "references"): boolean;
  hover(offset: number): Promise<LspHoverResult | undefined>;
  locations(
    method: LocationMethod,
    offset: number,
    includeDeclaration?: boolean,
  ): Promise<LspLocationsResult | undefined>;
}

/** 뷰의 요청 길. LSP 기능 칸(열린 문서)과 외부 소스 탭이 넣는다. */
export const navSource = Facet.define<NavSource, NavSource | undefined>({
  combine: (values) => values[0],
});

export interface NavigationStrings {
  definitions(count: number): string;
  implementations(count: number): string;
  usages(name: string, count: number): string;
  none: string;
  pin(key: string): string;
}

/** 사용처 하나의 파일(「검색 결과」 툴 윈도우가 받는다). */
export interface UsageFile {
  readonly path: string;
  readonly worktree?: string;
  readonly token?: string;
  readonly lines: readonly {
    line: number;
    preview: string;
    previewStart: number;
    ranges: { start: number; end: number }[];
    more: boolean;
  }[];
}

/** 창의 것. 창이 `setNavigationHost` 로 넘긴다 — 시험 · 파일 창처럼 없으면 위치를 열지 않는다. */
export interface NavigationHost {
  navigateTo(target: NavTarget, options: { record: boolean }): Promise<boolean>;
  strings: NavigationStrings;
  /** 위치 팝업의 고정 키를 Mac 표기로(`⌘↩`) 보일지. */
  mac: boolean;
  /** 사용처를 「검색 결과」 툴 윈도우에 고정한다. */
  pinUsages?(title: string, files: readonly UsageFile[]): void;
  /** 외부 소스 탭이 쓸 토큰의 머리 정보(경로 · 출처 서버 · 언어)를 적는다. */
  rememberToken?(
    token: string,
    meta: { path: string; server: string; language: string | null },
  ): void;
}

let host: NavigationHost | undefined;

/** 창이 넘긴다. 돌려준 함수가 거둔다. */
export function setNavigationHost(next: NavigationHost): () => void {
  host = next;
  return () => {
    if (host === next) host = undefined;
  };
}

export function navigationHost(): NavigationHost | undefined {
  return host;
}

// ---- 요청 길 -------------------------------------------------------------------------------------------------------------------

/** 열린 문서의 요청 길: 요청 도우미로(같은 종류의 앞 요청 취소 · 지난 답 버림 · 조합 중에는 묻지 않음). */
export function documentSource(doc: LspDocument): NavSource {
  // 능력 칸은 문서가 늘 지금 것을 든다(./features.ts 의 뷰 필드와 같은 값). features.ts 를 값으로 들이지 않는다 — 그 표가 이
  // 모듈을 들이므로 순환하면 표에 `undefined` 가 든다.
  const caps = () => doc.capabilities;
  const position = (offset: number) => (at: { docId: string; version: number }) =>
    ({ docId: at.docId, version: at.version, offset }) satisfies LspPositionParams;
  return {
    get language() {
      return doc.language;
    },
    services: doc.services,
    path: doc.path,
    ...(doc.worktree !== undefined && { worktree: doc.worktree }),
    offers: (feature) => caps()[feature] !== undefined,
    hover: (offset) => doc.call("hover", "lsp.hover", position(offset)),
    locations: (method, offset, includeDeclaration = false) =>
      method === "lsp.references"
        ? doc.call(
            method,
            method,
            (at) =>
              ({
                docId: at.docId,
                version: at.version,
                offset,
                includeDeclaration,
              }) satisfies LspReferencesParams,
          )
        : doc.call(method, method, position(offset)),
  };
}

/** 외부 소스(토큰)의 요청 길: 같은 서버로 — 같은 종류의 앞 요청은 취소한다. 실패(서버 끝남 · 토큰 무효)는 조용히 `undefined`. */
export function tokenSource(spec: {
  token: string;
  path: string;
  language: string | null;
  services: LspServices;
}): NavSource {
  const pending = new Map<string, AbortController>();
  async function ask<T>(kind: string, run: (signal: AbortSignal) => Promise<T>) {
    pending.get(kind)?.abort();
    const controller = new AbortController();
    pending.set(kind, controller);
    try {
      const answer = await run(controller.signal);
      return controller.signal.aborted ? undefined : answer;
    } catch {
      return undefined;
    } finally {
      if (pending.get(kind) === controller) pending.delete(kind);
    }
  }
  const { token, services } = spec;
  return {
    language: spec.language,
    services,
    path: spec.path,
    token,
    offers: () => true,
    hover: (offset) =>
      ask("hover", (signal) => services.call("lsp.hover", { token, offset }, { signal })),
    locations: (method, offset, includeDeclaration = false) =>
      ask(method, (signal) =>
        method === "lsp.references"
          ? services.call(method, { token, offset, includeDeclaration }, { signal })
          : services.call(method, { token, offset }, { signal }),
      ),
  };
}

// ---- 위치 열기 -----------------------------------------------------------------------------------------------------------------

/** 위치를 위치로 가기의 목적지로. 루트 안이면 그 문서의 워크트리, 루트 밖이면 토큰. */
export function targetOf(loc: LspLocation, source: NavSource): NavTarget {
  return {
    path: loc.path,
    line: loc.line,
    column: loc.column,
    ...(loc.token
      ? { token: loc.token }
      : source.worktree !== undefined && { worktree: source.worktree }),
  };
}

function remember(result: LspLocationsResult, source: NavSource) {
  for (const loc of result.locations)
    if (loc.token)
      host?.rememberToken?.(loc.token, {
        path: loc.path,
        server: result.server ?? "",
        language: source.language,
      });
}

function go(loc: LspLocation, source: NavSource) {
  void host?.navigateTo(targetOf(loc, source), { record: true });
}

/** 커서(`offset`)가 위치 `loc` 의 범위 안인가 — 「선언 위」. */
export function containsOffset(
  view: EditorView,
  source: NavSource,
  loc: LspLocation,
  offset: number,
) {
  const same = loc.token ? loc.token === source.token : !source.token && loc.path === source.path;
  if (!same) return false;
  const line = view.state.doc.lineAt(offset);
  const column = offset - line.from + 1;
  const after = line.number > loc.line || (line.number === loc.line && column >= loc.column);
  const before =
    line.number < loc.endLine || (line.number === loc.endLine && column <= loc.endColumn);
  return after && before;
}

/** 위치의 낱말 범위(없으면 그 자리). */
function wordRange(view: EditorView, offset: number) {
  const word = view.state.wordAt(offset);
  return word ? { from: word.from, to: word.to } : { from: offset, to: offset };
}

/** 선언으로 이동(`⌘B` · `⌘+클릭`). */
export async function gotoDeclaration(view: EditorView, offset: number) {
  const source = view.state.facet(navSource);
  if (!source?.offers("definition")) return;
  const result = await source.locations("lsp.definition", offset);
  if (!result || result.locations.length === 0) return;
  remember(result, source);
  if (result.locations.some((loc) => containsOffset(view, source, loc, offset))) {
    await findUsages(view, offset);
    return;
  }
  if (result.locations.length === 1) go(result.locations[0]!, source);
  else showLocations(view, offset, result.locations, source, "definitions");
}

/** 구현으로 이동(`⌘⌥B` · `⌘⌥+클릭`). */
export async function gotoImplementation(view: EditorView, offset: number) {
  const source = view.state.facet(navSource);
  if (!source?.offers("implementation")) return;
  const result = await source.locations("lsp.implementation", offset);
  if (!result || result.locations.length === 0) return;
  remember(result, source);
  if (result.locations.length === 1) go(result.locations[0]!, source);
  else showLocations(view, offset, result.locations, source, "implementations");
}

/** 사용처(`⌥F7`): 위치 팝업. 선언은 넣지 않는다. */
export async function findUsages(view: EditorView, offset: number) {
  const source = view.state.facet(navSource);
  if (!source?.offers("references")) return;
  const result = await source.locations("lsp.references", offset, false);
  if (!result) return;
  remember(result, source);
  showLocations(view, offset, result.locations, source, "usages");
}

/** 빠른 문서(`F1`): 커서 자리의 hover 를 고정 팝업으로. */
export async function quickDocumentation(view: EditorView, offset: number) {
  const source = view.state.facet(navSource);
  if (!source?.offers("hover")) return;
  const result = await source.hover(offset);
  const hover = result?.hover;
  if (!hover || view.state.selection.main.head !== offset) return;
  openPopup(view, hoverSpec(view, offset, hover, source, true));
}

function hoverSpec(
  view: EditorView,
  offset: number,
  hover: NonNullable<LspHoverResult["hover"]>,
  source: NavSource,
  pinned: boolean,
): PopupSpec {
  const word = wordRange(view, offset);
  const from = hover.from ?? word.from;
  return {
    kind: "hover",
    pos: Math.min(from, view.state.doc.length),
    body:
      hover.kind === "markdown"
        ? {
            kind: "markdown",
            markdown: hover.value,
            ...(source.language && { language: source.language }),
          }
        : { kind: "text", text: hover.value },
    data: { pinned, from, to: hover.to ?? word.to },
  };
}

// ---- 위치 팝업 -----------------------------------------------------------------------------------------------------------------

type ListKind = "definitions" | "implementations" | "usages";

function fileName(path: string) {
  return path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1) || path;
}

function dirName(path: string) {
  const at = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return at > 0 ? path.slice(0, at) : "";
}

/** 사용처를 파일별로 모은다(「검색 결과」 툴 윈도우의 꼴). */
export function usageFiles(locations: readonly LspLocation[], source: NavSource): UsageFile[] {
  const files = new Map<string, UsageFile & { lines: UsageFile["lines"][number][] }>();
  for (const loc of locations) {
    const key = loc.token ?? loc.path;
    let file = files.get(key);
    if (!file) {
      file = {
        path: loc.path,
        ...(loc.token
          ? { token: loc.token }
          : source.worktree !== undefined && { worktree: source.worktree }),
        lines: [],
      };
      files.set(key, file);
    }
    const preview = loc.preview ?? "";
    const start = Math.min(loc.column - 1, preview.length);
    const end =
      loc.endLine === loc.line ? Math.min(loc.endColumn - 1, preview.length) : preview.length;
    file.lines.push({
      line: loc.line,
      preview,
      previewStart: 0,
      ranges: [{ start, end: Math.max(start, end) }],
      more: false,
    });
  }
  for (const file of files.values()) file.lines.sort((a, b) => a.line - b.line);
  return [...files.values()];
}

/** 위치 팝업을 띄운다: 파일 · 줄 · 줄 글. 목록이 포커스를 가져가 `↑` `↓` `↩` 를 받고, 사용처는 `⌘↩` 로 고정한다. */
export function showLocations(
  view: EditorView,
  offset: number,
  locations: readonly LspLocation[],
  source: NavSource,
  kind: ListKind,
) {
  const strings = host?.strings;
  const word = wordRange(view, offset);
  const name = view.state.sliceDoc(word.from, word.to);
  const title =
    kind === "definitions"
      ? strings?.definitions(locations.length)
      : kind === "implementations"
        ? strings?.implementations(locations.length)
        : strings?.usages(name, locations.length);
  const pinKey = host?.mac ? "⌘↩" : "Ctrl+Enter";
  openPopup(view, {
    kind: "locations",
    pos: word.from,
    data: { kind, locations },
    body: {
      kind: "dom",
      render(parent, popupView) {
        parent.classList.add("pw-lsp-locations");
        const head = document.createElement("div");
        head.className = "pw-lsp-locations-title";
        head.textContent = title ?? "";
        parent.append(head);
        if (locations.length === 0) {
          const empty = document.createElement("div");
          empty.className = "pw-lsp-locations-empty";
          empty.textContent = strings?.none ?? "";
          parent.append(empty);
          return;
        }
        const list = document.createElement("ul");
        list.className = "pw-lsp-locations-list";
        list.setAttribute("role", "listbox");
        list.setAttribute("aria-label", title ?? "");
        list.tabIndex = 0;
        let selected = 0;
        const rows = locations.map((loc, index) => {
          const row = document.createElement("li");
          row.className = "pw-lsp-location";
          row.setAttribute("role", "option");
          row.id = `${parent.closest(".pw-lsp-popup")?.id ?? "pw-lsp-locations"}-${index}`;
          row.dataset.location = `${loc.path}:${loc.line}`;
          const place = document.createElement("span");
          place.className = "pw-lsp-location-place";
          place.textContent = `${fileName(loc.path)}:${loc.line}`;
          const dir = document.createElement("span");
          dir.className = "pw-lsp-location-dir";
          dir.textContent = dirName(loc.path);
          dir.title = loc.path;
          const text = document.createElement("span");
          text.className = "pw-lsp-location-text";
          // 서버가 준 줄 글은 글자로만 넣는다.
          text.textContent = loc.preview?.trim() ?? "";
          row.append(place, text, dir);
          row.addEventListener("mousedown", (e) => e.preventDefault());
          row.addEventListener("click", () => open(index));
          list.append(row);
          return row;
        });
        const mark = () => {
          rows.forEach((row, i) => row.setAttribute("aria-selected", String(i === selected)));
          list.setAttribute("aria-activedescendant", rows[selected]?.id ?? "");
          rows[selected]?.scrollIntoView?.({ block: "nearest" });
        };
        const open = (index: number) => {
          const loc = locations[index];
          if (!loc) return;
          closePopup(popupView, "main");
          go(loc, source);
        };
        list.addEventListener("keydown", (e) => {
          if (e.isComposing) return;
          const mod = host?.mac ? e.metaKey : e.ctrlKey;
          if (e.key === "ArrowDown") selected = Math.min(rows.length - 1, selected + 1);
          else if (e.key === "ArrowUp") selected = Math.max(0, selected - 1);
          else if (e.key === "Enter" && mod && kind === "usages" && host?.pinUsages) {
            closePopup(popupView, "main");
            host.pinUsages(title ?? name, usageFiles(locations, source));
          } else if (e.key === "Enter" && !mod) open(selected);
          else return;
          // 키맵(창의 단축키)이 같은 눌림을 보지 않게 한다. Esc 는 팝업 바탕이 받는다.
          e.preventDefault();
          e.stopPropagation();
          mark();
        });
        parent.append(list);
        if (kind === "usages" && host?.pinUsages) {
          const hint = document.createElement("div");
          hint.className = "pw-lsp-locations-hint";
          hint.textContent = strings?.pin(pinKey) ?? "";
          parent.append(hint);
        }
        mark();
        // 목록이 그려진 뒤 포커스를 가져간다 — 떠 있는 동안의 키가 밑의 문서로 가지 않게.
        requestAnimationFrame(() => {
          if (list.isConnected) list.focus({ preventScroll: true });
        });
      },
    },
  });
}

// ---- 마우스: 호버 · ⌘+호버 · ⌘+클릭 -------------------------------------------------------------------------------------------

/** ⌘(Win/Linux Ctrl) — 마우스로 선언에 가는 수정 키. */
function modKey(e: MouseEvent | KeyboardEvent) {
  return host?.mac === false ? e.ctrlKey : e.metaKey;
}

const setLinkRange = StateEffect.define<{ from: number; to: number } | null>();

/** ⌘+호버 밑줄. */
const linkField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    value = value.map(tr.changes);
    for (const e of tr.effects)
      if (e.is(setLinkRange)) {
        const builder = new RangeSetBuilder<Decoration>();
        if (e.value && e.value.to > e.value.from)
          builder.add(e.value.from, e.value.to, Decoration.mark({ class: "pw-lsp-link" }));
        value = builder.finish();
      }
    return value;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** 선언 미리보기의 본문: 정의의 앞 몇 줄을 코드 블록으로(언어는 이 문서의 것). 백틱이 든 줄도 깨지지 않게 울타리를 늘린다. */
export function previewMarkdown(lines: string, language: string | null) {
  const longest = Math.max(2, ...[...lines.matchAll(/`+/g)].map((m) => m[0].length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${language ?? ""}\n${lines}\n${fence}`;
}

/** 위치의 앞 몇 줄을 읽는다: 루트 안은 `fs.read`, 루트 밖은 토큰의 `lsp.document.read`. 못 읽으면 위치의 한 줄. */
async function previewLines(loc: LspLocation, source: NavSource, projectId: string | undefined) {
  const cut = (text: string) =>
    text
      .split(/\r\n|\r|\n/)
      .slice(loc.line - 1, loc.line - 1 + PREVIEW_LINES)
      .join("\n")
      .replace(/\n+$/, "");
  try {
    if (loc.token)
      return cut((await source.services.call("lsp.document.read", { token: loc.token })).text);
    if (projectId !== undefined) {
      const read = await source.services.call("fs.read", {
        projectId,
        worktree: source.worktree ?? null,
        path: loc.path,
      });
      if (read.content.kind === "text") return cut(read.content.text);
    }
  } catch {
    // 아래의 한 줄로 물러난다.
  }
  return loc.preview ?? undefined;
}

class Hovering implements PluginValue {
  private timer: ReturnType<typeof setTimeout> | undefined;
  /** 마지막 마우스 자리(문서 위치)와 그때의 수정 키. */
  private at: { pos: number; mod: boolean; from: number; to: number } | undefined;
  private readonly onMove = (e: MouseEvent) => this.move(e);
  private readonly onLeave = (e: MouseEvent) => this.leave(e);
  private readonly onKey = (e: KeyboardEvent) => this.key(e);

  constructor(
    private readonly view: EditorView,
    private readonly projectId: string | undefined,
  ) {
    view.contentDOM.addEventListener("mousemove", this.onMove);
    view.dom.addEventListener("mouseleave", this.onLeave);
    window.addEventListener("keydown", this.onKey);
    window.addEventListener("keyup", this.onKey);
  }

  private source() {
    return this.view.state.facet(navSource);
  }

  private move(e: MouseEvent) {
    const pos = this.view.posAtCoords({ x: e.clientX, y: e.clientY }, false);
    const mod = modKey(e);
    const word = this.view.state.wordAt(pos);
    // 팝업이 가리킨 낱말 밖으로 나가면(고정 팝업이 아니면) 닫는다.
    const popup = currentPopup(this.view, "main");
    const data =
      popup?.kind === "hover"
        ? (popup.data as { pinned?: boolean; from: number; to: number })
        : undefined;
    if (data && !data.pinned && (pos < data.from || pos > data.to)) closePopup(this.view, "main");
    if (!word) {
      this.clear();
      return;
    }
    if (this.at && this.at.from === word.from && this.at.to === word.to && this.at.mod === mod)
      return;
    this.clear();
    this.at = { pos, mod, from: word.from, to: word.to };
    if (mod) this.view.dispatch({ effects: setLinkRange.of(word) });
    const at = this.at;
    this.timer = setTimeout(() => void this.fire(at), HOVER_DELAY_MS);
  }

  private async fire(at: { pos: number; mod: boolean; from: number; to: number }) {
    this.timer = undefined;
    const source = this.source();
    if (!source) return;
    if (at.mod) {
      if (!source.offers("definition")) return;
      const result = await source.locations("lsp.definition", at.pos);
      const loc = result?.locations[0];
      if (!loc || this.at !== at) return;
      remember(result!, source);
      const lines = await previewLines(loc, source, this.projectId);
      if (lines === undefined || this.at !== at) return;
      openPopup(this.view, {
        kind: "hover",
        pos: at.from,
        body: { kind: "markdown", markdown: previewMarkdown(lines, source.language) },
        data: { pinned: false, from: at.from, to: at.to, preview: true },
      });
      return;
    }
    if (!source.offers("hover")) return;
    const result = await source.hover(at.pos);
    // 답을 기다리는 사이 마우스가 다른 낱말로 갔으면 버린다.
    if (!result?.hover || this.at !== at) return;
    openPopup(this.view, hoverSpec(this.view, at.pos, result.hover, source, false));
  }

  private leave(e: MouseEvent) {
    // 팝업 안으로 들어간 것은 떠난 것이 아니다.
    const into = e.relatedTarget instanceof Element && e.relatedTarget.closest(".pw-lsp-popup");
    if (into) return;
    this.clear();
    const popup = currentPopup(this.view, "main");
    if (popup?.kind === "hover" && !(popup.data as { pinned?: boolean } | undefined)?.pinned)
      closePopup(this.view, "main");
  }

  private key(e: KeyboardEvent) {
    if (e.key !== "Meta" && e.key !== "Control") return;
    // 수정 키를 떼면 밑줄을 걷는다(누르면 다음 마우스 움직임이 긋는다).
    if (e.type === "keyup" && this.at?.mod) this.clear();
  }

  private clear() {
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.at?.mod) this.view.dispatch({ effects: setLinkRange.of(null) });
    this.at = undefined;
  }

  destroy() {
    clearTimeout(this.timer);
    this.view.contentDOM.removeEventListener("mousemove", this.onMove);
    this.view.dom.removeEventListener("mouseleave", this.onLeave);
    window.removeEventListener("keydown", this.onKey);
    window.removeEventListener("keyup", this.onKey);
  }
}

/** ⌘+클릭(Win/Linux Ctrl+클릭) 선언으로, ⌘⌥+클릭 구현으로. CodeMirror 의 클릭(커서 두기)보다 먼저 받는다. */
const clicks = EditorView.domEventHandlers({
  mousedown(e, view) {
    if (e.button !== 0 || !modKey(e) || e.shiftKey) return false;
    const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
    if (pos === null) return false;
    e.preventDefault();
    view.dispatch({ effects: setLinkRange.of(null) });
    void (e.altKey ? gotoImplementation(view, pos) : gotoDeclaration(view, pos));
    return true;
  },
});

const theme = EditorView.baseTheme({
  ".pw-lsp-link": { textDecoration: "underline", cursor: "pointer" },
  ".pw-lsp-locations": { padding: "0 !important", minWidth: "360px" },
  ".pw-lsp-locations-title": {
    padding: "4px 10px",
    fontWeight: "600",
    borderBottom: "1px solid var(--pw-popup-border)",
  },
  ".pw-lsp-locations-empty": { padding: "6px 10px" },
  ".pw-lsp-locations-list": {
    listStyle: "none",
    margin: "0",
    padding: "2px 0",
    maxHeight: "min(320px, 45vh)",
    overflow: "auto",
    outline: "none",
  },
  ".pw-lsp-location": {
    display: "flex",
    gap: "8px",
    padding: "1px 10px",
    whiteSpace: "nowrap",
    cursor: "default",
  },
  ".pw-lsp-location[aria-selected=true]": { backgroundColor: "var(--pw-ui-active)" },
  ".pw-lsp-location-place": { flex: "none" },
  ".pw-lsp-location-text": {
    flex: "1 1 auto",
    minWidth: "0",
    overflow: "hidden",
    textOverflow: "ellipsis",
    fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)",
  },
  ".pw-lsp-location-dir": {
    flex: "0 1 auto",
    minWidth: "0",
    overflow: "hidden",
    textOverflow: "ellipsis",
    opacity: "0.7",
  },
  ".pw-lsp-locations-hint": {
    padding: "2px 10px 4px",
    borderTop: "1px solid var(--pw-popup-border)",
    opacity: "0.7",
  },
});

/** 요청 길 `source` 의 내비게이션 확장(LSP 기능 칸 · 외부 소스 탭이 함께 쓴다). */
export function navigationExtension(source: NavSource, projectId: string | undefined) {
  return [
    navSource.of(source),
    linkField,
    ViewPlugin.define((view) => new Hovering(view, projectId)),
    clicks,
    theme,
  ];
}

/** LSP 기능 표의 한 줄(./features.ts). */
export const navigationFeature: LspEditorFeature = {
  id: "navigation",
  extension: (_context, doc) => navigationExtension(documentSource(doc), doc.projectId),
};
