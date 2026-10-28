// diff 의 hunk · 줄 체크 = 부분 스테이징(docs/design/13-m2-plan.md M2-16 · 10-ui.md 「주요 툴 윈도우 상세」 의 커밋): M2-15 의 diff 뷰가
// 열어 둔 확장 지점(`DiffViewSlots.hunkControls`)에 끼우는 CodeMirror 거터. 커밋 툴 윈도우의 미리보기(staging-changes.tsx)가 쓴다.
//
// - 변경 묶음(문맥 줄 없이 이어진 뺀 줄 · 더한 줄 — diff-model.ts 의 `DiffGroup` 과 같은 차례)마다 묶음 체크, 바뀐 줄마다 줄 체크를
//   단다. 누르면 곧바로 쓴다: 작업 트리 diff(index ↔ 작업 트리)는 체크가 비어 있고 누르면 스테이징(`git.stageLines`), 스테이징된
//   diff(HEAD ↔ index)는 체크돼 있고 누르면 스테이징을 푼다(`git.unstageLines`). 쓴 뒤에는 diff 를 다시 읽는다 — 그 줄은 이 diff 에서
//   빠진다. 체크 자체는 누를 때 바뀌지 않는다(다시 읽은 diff 가 상태다).
// - 요청은 `git.diff` 가 준 hunk 그대로와 그 안의 줄 인덱스다(`HunkSelection`) — Host 가 지금 diff 에서 같은 hunk 를 찾지 못하면
//   (그사이 파일이 바뀜) 아무것도 쓰지 않는다.
// - 나란히 보기: 새 쪽에 더한 줄의 체크와 묶음 체크, 옛 쪽에 뺀 줄의 체크(빼기만인 묶음은 묶음 체크도 옛 쪽). 통합 보기: 새 쪽 문서에
//   더한 줄의 체크와 묶음 체크 — 뺀 줄은 블록 위젯이라 줄마다 고르지 못한다(묶음으로 고르거나 나란히 보기에서).
// - 작업 트리 diff 는 묶음 체크 옆에 되돌리기(↶)를 둔다 — 그 묶음을 작업 트리에서 버린다(`git.discardLines`, 확인 뒤 — 결정 M2-D6 ①).
// - 공백 무시로 받은 hunk 는 실제 줄과 맞지 않으므로 칸을 달지 않는다(부르는 쪽이 `ignoreWhitespace` 면 확장을 넘기지 않는다).

import { type Extension, RangeSet, type Text } from "@codemirror/state";
import { EditorView, gutter, GutterMarker } from "@codemirror/view";
import type { DiffFile, DiffHunk } from "@pitwall/rpc-client";

import type { DiffSlotContext, DiffViewSlots } from "../diff/diff-view";

/** 변경 묶음 하나와 그것이 든 hunk 의 줄 인덱스. */
export interface StageGroup {
  /** `file.hunks` 의 인덱스. */
  readonly hunk: number;
  /** 묶음의 바뀐 줄(`hunk.lines` 의 인덱스). */
  readonly lines: readonly number[];
  /** 새 쪽 줄 번호 → 줄 인덱스(더한 줄). */
  readonly added: ReadonlyMap<number, number>;
  /** 옛 쪽 줄 번호 → 줄 인덱스(뺀 줄). */
  readonly removed: ReadonlyMap<number, number>;
  /** 새 쪽에서 묶음이 시작하는 줄(더한 줄이 없으면 끼어드는 자리 — 그 줄 앞). */
  readonly newFrom: number;
  readonly oldFrom: number;
}

/** 파일의 hunk 를 변경 묶음으로 나눈다(diff-model.ts `buildDiffModel` 과 같은 차례). */
export function stageGroups(file: Pick<DiffFile, "hunks">): StageGroup[] {
  const out: StageGroup[] = [];
  file.hunks.forEach((hunk, h) => {
    let nextOld = hunk.oldLines === 0 ? hunk.oldStart + 1 : hunk.oldStart;
    let nextNew = hunk.newLines === 0 ? hunk.newStart + 1 : hunk.newStart;
    let open:
      | {
          lines: number[];
          added: Map<number, number>;
          removed: Map<number, number>;
          newFrom: number;
          oldFrom: number;
        }
      | undefined;
    const close = () => {
      if (open) out.push({ hunk: h, ...open });
      open = undefined;
    };
    hunk.lines.forEach((line, i) => {
      if (line.kind === "context") {
        close();
        nextOld = (line.oldLine ?? nextOld) + 1;
        nextNew = (line.newLine ?? nextNew) + 1;
        return;
      }
      open ??= {
        lines: [],
        added: new Map(),
        removed: new Map(),
        newFrom: nextNew,
        oldFrom: nextOld,
      };
      open.lines.push(i);
      if (line.kind === "removed") {
        const n = line.oldLine ?? nextOld;
        open.removed.set(n, i);
        nextOld = n + 1;
      } else {
        const n = line.newLine ?? nextNew;
        open.added.set(n, i);
        nextNew = n + 1;
      }
    });
    close();
  });
  return out;
}

/** 고른 것: hunk 그대로와 그 안의 줄. */
export interface StageSelection {
  readonly hunk: DiffHunk;
  readonly lines: readonly number[];
}

export interface StagingSlotOptions {
  /** `stage`: 작업 트리 diff(체크가 비어 있다), `unstage`: 스테이징된 diff(체크돼 있다). */
  readonly mode: "stage" | "unstage";
  /** 체크를 눌렀다. */
  toggle(selection: StageSelection): void;
  /** 묶음 되돌리기(↶)를 눌렀다 — `stage` 모드에만 있다. */
  revert?(selection: StageSelection): void;
  readonly labels: { readonly group: string; readonly line: string; readonly revert: string };
}

class StageMarker extends GutterMarker {
  constructor(
    private readonly key: string,
    private readonly build: () => HTMLElement,
  ) {
    super();
  }

  override eq(other: GutterMarker): boolean {
    return other instanceof StageMarker && other.key === this.key;
  }

  override toDOM(): Node {
    return this.build();
  }
}

/** 줄 번호(1 기준)의 줄 시작 위치. 문서 밖이면 마지막 줄. */
function lineStart(doc: Text, line: number): number {
  return doc.line(Math.min(Math.max(1, line), doc.lines)).from;
}

/** 쪽 하나의 거터. */
function sideGutter(context: DiffSlotContext, options: StagingSlotOptions): Extension {
  const { file, side } = context;
  const groups = stageGroups(file);
  const doc = side === "a" ? context.a : context.b;
  const checked = options.mode === "unstage";
  /** 줄 번호 → 그 줄에 둘 것. */
  const at = new Map<number, { group?: number; line?: number; groupIndex: number }>();
  const put = (n: number, value: { group?: number; line?: number; groupIndex: number }) => {
    const prev = at.get(n);
    at.set(n, { ...prev, ...value });
  };
  groups.forEach((g, gi) => {
    if (side === "a") {
      for (const [n, i] of g.removed) put(n, { line: i, groupIndex: gi });
      if (g.added.size === 0) put(g.oldFrom, { group: gi, groupIndex: gi });
    } else {
      for (const [n, i] of g.added) put(n, { line: i, groupIndex: gi });
      // 새 쪽에 줄이 없는 묶음: 나란히 보기는 옛 쪽에 묶음 체크를 두고, 통합 보기는 끼어드는 자리(다음 줄)에 둔다.
      if (g.added.size > 0 || side === "unified")
        put(Math.min(g.newFrom, doc.lines), { group: gi, groupIndex: gi });
    }
  });

  const box = (label: string, attr: string, value: number, onClick: () => void) => {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = checked;
    input.className = "pw-stage-box";
    input.setAttribute(attr, String(value));
    input.setAttribute("aria-label", label);
    input.title = label;
    // 누른 때 체크를 바꾸지 않는다 — 다시 읽은 diff 가 상태다. 에디터 선택도 옮기지 않는다.
    input.addEventListener("mousedown", (e) => e.preventDefault());
    input.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      onClick();
    });
    return input;
  };
  const spacer = () => {
    const s = document.createElement("span");
    s.className = "pw-stage-spacer";
    return s;
  };
  const selection = (g: StageGroup, lines: readonly number[]): StageSelection => ({
    hunk: file.hunks[g.hunk]!,
    lines,
  });

  const markers = [...at.entries()]
    .sort(([x], [y]) => x - y)
    .map(([n, place]) => {
      const g = groups[place.groupIndex]!;
      const marker = new StageMarker(
        `${side}:${n}:${place.group ?? ""}:${place.line ?? ""}`,
        () => {
          const el = document.createElement("span");
          el.className = "pw-stage";
          el.setAttribute("data-stage-at", String(n));
          if (place.group !== undefined) {
            el.append(
              box(options.labels.group, "data-stage-group", place.group, () =>
                options.toggle(selection(g, g.lines)),
              ),
            );
            if (options.revert) {
              const revert = document.createElement("button");
              revert.type = "button";
              revert.className = "pw-stage-revert";
              revert.textContent = "↶";
              revert.setAttribute("data-stage-revert", String(place.group));
              revert.setAttribute("aria-label", options.labels.revert);
              revert.title = options.labels.revert;
              revert.addEventListener("mousedown", (e) => e.preventDefault());
              revert.addEventListener("click", (e) => {
                e.stopPropagation();
                options.revert?.(selection(g, g.lines));
              });
              el.append(revert);
            } else el.append(spacer());
          } else {
            el.append(spacer(), ...(options.revert ? [spacer()] : []));
          }
          if (place.line !== undefined) {
            const i = place.line;
            el.append(
              box(options.labels.line, "data-stage-line", i, () =>
                options.toggle(selection(g, [i])),
              ),
            );
          } else el.append(spacer());
          return el;
        },
      );
      return marker.range(lineStart(doc, n));
    });
  const set = RangeSet.of(markers);
  return gutter({
    class: "pw-stage-gutter",
    markers: () => set,
    initialSpacer: () =>
      new StageMarker("spacer", () => {
        const el = document.createElement("span");
        el.className = "pw-stage";
        el.append(spacer(), ...(options.revert ? [spacer()] : []), spacer());
        return el;
      }),
  });
}

/** 칸의 모양: 체크 두 칸(묶음 · 줄)과 되돌리기 단추가 한 줄 높이 안에 든다. */
const stagingTheme = EditorView.baseTheme({
  ".pw-stage-gutter .cm-gutterElement": { display: "flex", alignItems: "center" },
  ".pw-stage": { display: "inline-flex", alignItems: "center", gap: "2px", padding: "0 2px" },
  ".pw-stage-box": { width: "12px", height: "12px", margin: "0", cursor: "pointer" },
  ".pw-stage-spacer": { display: "inline-block", width: "12px" },
  ".pw-stage-revert": {
    width: "12px",
    padding: "0",
    border: "none",
    background: "none",
    color: "inherit",
    cursor: "pointer",
    fontSize: "11px",
    lineHeight: "1",
  },
});

/** diff 뷰의 확장 지점에 넘길 것. 같은 `options` 면 같은 객체를 쓴다(부르는 쪽이 memo 한다). */
export function stagingSlots(options: StagingSlotOptions): DiffViewSlots {
  return { hunkControls: (context) => [stagingTheme, sideGutter(context, options)] };
}
