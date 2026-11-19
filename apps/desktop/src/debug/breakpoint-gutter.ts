// 에디터의 브레이크포인트 거터 · 멈춘 줄(17-m6-plan M6-18 ① ③). 줄 번호 왼쪽 칸을 누르면 그 줄의 브레이크포인트를 켜고 끄고,
// 우클릭하면 그 줄의 편집 팝업(조건 · 로그 메시지 · hit count · 끄기)을 연다.
//
// - 위치의 주인은 창의 `Breakpoints`(./breakpoints.ts — 문서마다 하나)다. 뷰는 같은 위치를 자기 필드(`marks`)에 쥐고 편집이 오면
//   **같은 매핑**(`mapPos(pos, 1)` → 줄 맨 앞)으로 옮긴다 — 문서 쪽 바뀜 알림은 뷰가 갱신된 뒤에 오므로 거기에 기대지 않는다.
//   누름 · Host 이벤트처럼 편집 밖에서 바뀐 것은 `Breakpoints.onMoved` 로 받아 다시 읽는다.
// - 멈춘 줄: 창의 디버그 세션이 고른 프레임이 이 문서면 그 줄에 배경과 거터 화살표를 그린다(`current`).

import {
  type EditorState,
  type Extension,
  Facet,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  ViewPlugin,
} from "@codemirror/view";
import type { Breakpoint } from "@pitwall/rpc-client";
import type { StoreApi } from "zustand/vanilla";

import type { Breakpoints, Placed } from "./breakpoints";

/** 멈춘 자리(창의 디버그 세션이 고른 프레임). */
export interface CurrentFrame {
  /** 실제 절대 경로(문서 키와 같은 꼴). */
  path: string;
  line: number;
}

/** 에디터 확장이 받는 디버그 쪽(`EditorFeatureContext.debug`). */
export interface EditorDebug {
  readonly breakpoints: Breakpoints;
  /** 문서를 브레이크포인트에 붙인다(루트를 풀어 상대 경로를 정한다). 놓는 함수. */
  attach(path: string, worktree: string | undefined): () => void;
  readonly current: StoreApi<{ frame: CurrentFrame | undefined }>;
  /** 줄의 편집 팝업을 연다(화면 좌표). */
  editBreakpoint(path: string, line: number, at: { x: number; y: number }): void;
  /** 표시의 풍선 도움말. */
  describe(bp: Breakpoint, unverified: string | undefined): string;
}

const setMarks = StateEffect.define<readonly Placed[]>();
/** 이 상태의 거터 표시 필드(시험 · 진단이 줄을 읽는다 — `gutterLines`). */
const marksFields = Facet.define<StateField<RangeSet<GutterMarker>>>();

/** 뷰의 거터에 보이는 브레이크포인트 줄(1부터, 줄 차례). */
export function gutterLines(state: EditorState): number[] {
  const field = state.facet(marksFields)[0];
  if (!field) return [];
  const out: number[] = [];
  for (let c = state.field(field).iter(); c.value; c.next())
    out.push(state.doc.lineAt(c.from).number);
  return out;
}
const setCurrent = StateEffect.define<number | null>();

/** `agent` 칸(M6-20 이 더한다 — 없으면 거짓). */
function byAgent(bp: Breakpoint): boolean {
  return (bp as { agent?: boolean }).agent === true;
}

class BreakpointMarker extends GutterMarker {
  constructor(
    readonly bp: Breakpoint,
    readonly unverified: string | undefined,
    readonly title: string,
  ) {
    super();
  }

  override eq(other: GutterMarker): boolean {
    return (
      other instanceof BreakpointMarker &&
      other.title === this.title &&
      other.unverified === this.unverified &&
      (other.bp.enabled ?? true) === (this.bp.enabled ?? true) &&
      byAgent(other.bp) === byAgent(this.bp) &&
      kind(other.bp) === kind(this.bp)
    );
  }

  override toDOM(): Node {
    const el = document.createElement("span");
    const classes = ["cm-breakpoint", `cm-breakpoint-${kind(this.bp)}`];
    if (this.bp.enabled === false) classes.push("cm-breakpoint-disabled");
    if (this.unverified !== undefined) classes.push("cm-breakpoint-unverified");
    if (byAgent(this.bp)) classes.push("cm-breakpoint-agent");
    el.className = classes.join(" ");
    el.dataset.breakpoint = kind(this.bp);
    el.title = this.title;
    return el;
  }
}

class CurrentMarker extends GutterMarker {
  override toDOM(): Node {
    const el = document.createElement("span");
    el.className = "cm-debug-current-arrow";
    el.textContent = "▶";
    return el;
  }
}

/** 표시의 종류: 로그포인트 · 조건(hit count 포함) · 그냥. */
function kind(bp: Breakpoint): "log" | "condition" | "plain" {
  if (bp.logMessage) return "log";
  if (bp.condition || bp.hitCondition) return "condition";
  return "plain";
}

const currentLine = Decoration.line({ class: "cm-debug-current-line" });
const currentArrow = new CurrentMarker();

const spacer = new (class extends GutterMarker {
  override toDOM() {
    const el = document.createElement("span");
    el.className = "cm-breakpoint-spacer";
    return el;
  }
})();

/** 이 문서의 브레이크포인트 거터 · 멈춘 줄. */
export function breakpointGutter(
  debug: EditorDebug,
  path: string,
  worktree: string | undefined,
): Extension {
  const markers = (items: readonly Placed[]) => {
    const rel = debug.breakpoints.relative(path);
    const unverified = debug.breakpoints.store.getState().unverified;
    const b = new RangeSetBuilder<GutterMarker>();
    for (const { pos, bp } of items) {
      const why = rel === undefined ? undefined : unverified[`${rel}:${bp.line}`];
      b.add(pos, pos, new BreakpointMarker(bp, why, debug.describe(bp, why)));
    }
    return b.finish();
  };

  const marks = StateField.define<RangeSet<GutterMarker>>({
    create: () => markers(debug.breakpoints.placed(path)),
    update(value, tr) {
      for (const e of tr.effects) if (e.is(setMarks)) return markers(e.value);
      if (!tr.docChanged) return value;
      // 줄 맨 앞으로 맞추고 같은 줄은 하나만(./breakpoints.ts `mapPlaced` 와 같은 규칙).
      const b = new RangeSetBuilder<GutterMarker>();
      const seen = new Set<number>();
      const cursor = value.iter();
      const moved: [number, GutterMarker][] = [];
      for (; cursor.value; cursor.next())
        moved.push([tr.state.doc.lineAt(tr.changes.mapPos(cursor.from, 1)).from, cursor.value]);
      moved.sort((x, y) => x[0] - y[0]);
      for (const [pos, m] of moved) {
        if (seen.has(pos)) continue;
        seen.add(pos);
        b.add(pos, pos, m);
      }
      return b.finish();
    },
  });

  const current = StateField.define<{ line: number | null; deco: DecorationSet }>({
    create: () => ({ line: null, deco: Decoration.none }),
    update(value, tr) {
      for (const e of tr.effects)
        if (e.is(setCurrent)) {
          const line = e.value;
          if (line === null || line < 1 || line > tr.state.doc.lines)
            return { line: null, deco: Decoration.none };
          const from = tr.state.doc.line(line).from;
          return { line, deco: Decoration.set([currentLine.range(from)]) };
        }
      return tr.docChanged ? { ...value, deco: value.deco.map(tr.changes) } : value;
    },
    provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
  });

  const link = ViewPlugin.define((view) => {
    let alive = true;
    const release = debug.attach(path, worktree);
    const refresh = () =>
      queueMicrotask(() => {
        if (alive) view.dispatch({ effects: setMarks.of(debug.breakpoints.placed(path)) });
      });
    const offMoved = debug.breakpoints.onMoved((p) => p === path && refresh());
    const offStore = debug.breakpoints.store.subscribe((s, prev) => {
      if (s.unverified !== prev.unverified) refresh();
    });
    const showCurrent = () =>
      queueMicrotask(() => {
        if (!alive) return;
        const frame = debug.current.getState().frame;
        const line = frame && frame.path === path ? frame.line : null;
        if (view.state.field(current).line !== line)
          view.dispatch({ effects: setCurrent.of(line) });
      });
    const offCurrent = debug.current.subscribe(showCurrent);
    showCurrent();
    refresh();
    return {
      destroy() {
        alive = false;
        offMoved();
        offStore();
        offCurrent();
        release();
      },
    };
  });

  const lineOf = (view: EditorView, from: number) => view.state.doc.lineAt(from).number;

  return [
    marks,
    marksFields.of(marks),
    current,
    link,
    gutter({
      class: "cm-debug-gutter",
      markers: (v) => {
        const line = v.state.field(current).line;
        if (line === null) return v.state.field(marks);
        const from = v.state.doc.line(line).from;
        return [v.state.field(marks), RangeSet.of([currentArrow.range(from)])];
      },
      initialSpacer: () => spacer,
      domEventHandlers: {
        mousedown(view, block, event) {
          const e = event as MouseEvent;
          if (e.button !== 0) return false;
          debug.breakpoints.toggle(path, lineOf(view, block.from));
          return true;
        },
        contextmenu(view, block, event) {
          const e = event as MouseEvent;
          e.preventDefault();
          debug.editBreakpoint(path, lineOf(view, block.from), { x: e.clientX, y: e.clientY });
          return true;
        },
      },
    }),
    EditorView.baseTheme({
      ".cm-debug-gutter": { width: "14px", cursor: "default" },
      ".cm-debug-gutter .cm-gutterElement": {
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      },
      ".cm-breakpoint": {
        display: "inline-block",
        width: "9px",
        height: "9px",
        borderRadius: "50%",
        background: "var(--pw-diagnostic-error, #d33)",
      },
      ".cm-breakpoint-condition": { borderRadius: "2px" },
      ".cm-breakpoint-log": {
        transform: "rotate(45deg)",
        borderRadius: "1px",
        width: "8px",
        height: "8px",
      },
      ".cm-breakpoint-disabled": {
        background: "transparent",
        border: "1px solid var(--pw-diagnostic-error, #d33)",
      },
      ".cm-breakpoint-unverified": { opacity: "0.5" },
      ".cm-breakpoint-agent": { outline: "1px dashed currentColor", outlineOffset: "1px" },
      ".cm-debug-current-line": {
        backgroundColor: "color-mix(in srgb, var(--pw-diagnostic-warning, #c90) 22%, transparent)",
      },
      ".cm-debug-current-arrow": {
        color: "var(--pw-diagnostic-warning, #c90)",
        fontSize: "9px",
        lineHeight: "1",
      },
    }),
  ];
}
