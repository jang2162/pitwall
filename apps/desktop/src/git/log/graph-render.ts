// Git 로그 목록의 canvas 그리기(docs/design/13-m2-plan.md 결정 M2-D2 — 자체 canvas 렌더러): 고정 행 높이, 화면에 든 행만 그린다.
// 한 행 = 그래프(레인 · 간선 · 노드) · ref 라벨 · 제목 · 작성자 · 날짜 · 짧은 해시. 그리기는 이 함수 하나이고 React 를 거치지 않는다
// — 스크롤마다 log-list.tsx 가 다음 프레임에 부른다.
//
// - 레인 색은 테마 토큰 `graph.lane.1~8` 을 색 번호의 나머지로 고른다(`graphLaneToken`). 접힌 레인의 색 번호(4294967295)는 나머지를
//   내기 전에 따로 보고 `graph.lane.folded` 로 칠한다(M2-03 재리뷰에서 넘어온 것 — 나머지로 고르면 `graph.lane.8` 과 같아진다).
// - 간선: 지나가는 레인은 행 높이 전체의 세로줄, 노드로 모이는 간선은 윗변의 그 레인에서 노드로, 노드에서 나가는 간선은 노드에서
//   아랫변의 그 레인으로 곧은 선이다(레인이 같으면 세로).
// - 레인 호버: 가리킨 간선의 색 번호와 같은 간선(첫 부모로 이어진 한 줄 — pw-git 이 줄마다 색 번호를 준다)을 굵게, 나머지를 흐리게
//   그린다(10-ui.md 「Git」 마우스 표 「해당 브랜치 경로 강조」). 접힌 레인은 강조하지 않는다(여러 갈래가 섞인 칸이다).
// - 시험은 `Ctx` 를 흉내 낸 기록기로 그린 것을 본다(happy-dom 에는 canvas 2D 가 없다).

import type { LogRef, LogRow } from "@pitwall/rpc-client";

import { GRAPH_LANES, graphLaneToken, type TokenValues } from "../../theme/tokens";
import { type Edge, FOLDED_COLOR, type RowLanes } from "./lanes";

export const ROW_HEIGHT = 22;
export const LANE_WIDTH = 12;
const GRAPH_PAD = 6;
const NODE_RADIUS = 4;
const TEXT_PAD = 8;
const LABEL_GAP = 4;
const LABEL_PAD = 4;
const LABEL_HEIGHT = 16;
const AUTHOR_WIDTH = 120;
const DATE_WIDTH = 120;
const HASH_WIDTH = 64;
/** 이보다 좁으면 작성자 · 날짜 칸을 감춘다. */
const NARROW = 520;

/** 그리기에 쓰는 canvas 2D 의 부분(시험이 흉내 낸다). */
export type Ctx = Pick<
  CanvasRenderingContext2D,
  | "beginPath"
  | "moveTo"
  | "lineTo"
  | "stroke"
  | "fill"
  | "arc"
  | "fillRect"
  | "strokeRect"
  | "fillText"
  | "measureText"
  | "save"
  | "restore"
  | "setTransform"
  | "clearRect"
  | "rect"
  | "clip"
> & {
  strokeStyle: CanvasRenderingContext2D["strokeStyle"];
  fillStyle: CanvasRenderingContext2D["fillStyle"];
  lineWidth: number;
  globalAlpha: number;
  font: string;
  textBaseline: CanvasTextBaseline;
  textAlign: CanvasTextAlign;
};

export interface Palette {
  lanes: readonly string[];
  folded: string;
  node: string;
  background: string;
  foreground: string;
  muted: string;
  selected: string;
  hover: string;
  border: string;
  accent: string;
}

/** 테마 값 → 그리기 색. 테마가 바뀌면 다시 만든다. */
export function paletteOf(values: TokenValues): Palette {
  return {
    lanes: Array.from({ length: GRAPH_LANES }, (_, i) => values[graphLaneToken(i)]),
    folded: values["graph.lane.folded"],
    node: values["graph.node"],
    background: values["ui.panelBackground"],
    foreground: values["ui.foreground"],
    muted: values["ui.mutedForeground"],
    selected: values["ui.activeBackground"],
    hover: values["ui.hoverBackground"],
    border: values["ui.border"],
    accent: values["ui.accent"],
  };
}

/** 색 번호의 색. 접힌 레인은 따로 본다(머리 주석). */
export function laneColor(palette: Palette, color: number): string {
  if (color === FOLDED_COLOR) return palette.folded;
  return palette.lanes[color % palette.lanes.length]!;
}

/** 레인의 가운데 x. */
export function laneX(lane: number): number {
  return GRAPH_PAD + lane * LANE_WIDTH + LANE_WIDTH / 2;
}

/**
 * 그래프 칸의 폭: 가장 오른쪽 레인 `widest` 까지, 목록 폭 `width` 의 절반을 넘지 않게(넘는 레인은 잘린다 — 레인 64 개를 다 그리면
 * 780 px 다, 결정 M2-D2). HEAD 의 줄은 레인 0 이라(pw-git) 잘려도 보인다. 레인이 없으면(걸러진 목록) 0.
 */
export function graphWidth(widest: number, width: number): number {
  if (widest < 0) return 0;
  const full = GRAPH_PAD * 2 + (widest + 1) * LANE_WIDTH;
  return Math.min(full, Math.max(GRAPH_PAD * 2 + LANE_WIDTH * 4, Math.floor(width / 2)));
}

/** x 가 가리키는 레인(그래프 칸 `graphPx` 밖이면 -1). */
export function laneAt(x: number, graphPx: number): number {
  if (x < GRAPH_PAD || x >= graphPx) return -1;
  return Math.floor((x - GRAPH_PAD) / LANE_WIDTH);
}

/** 행 `lanes` 에서 레인 `lane` 을 지나는 간선의 색 번호(없으면 `undefined`). 노드 · 지나가는 레인 · 간선 순으로 본다. */
export function colorAtLane(lanes: RowLanes, lane: number): number | undefined {
  const g = lanes.graph;
  if (g.lane === lane) return g.color;
  const find = (edges: readonly Edge[]) => edges.find((e) => e[0] === lane)?.[1];
  return find(lanes.pass) ?? find(g.up) ?? find(g.down);
}

/** 그린 ref 라벨 하나의 자리(끌기 · 누르기의 판정). */
export interface LabelHit {
  row: number;
  ref: LogRef;
  x: number;
  width: number;
}

export interface DrawInput {
  ctx: Ctx;
  width: number;
  height: number;
  scrollTop: number;
  rows: readonly LogRow[];
  /** `first` 행부터 화면에 든 행의 레인(`rows` 와 같은 차례). 걸러진 목록이면 원소가 `null`. */
  lanes: readonly (RowLanes | null)[];
  first: number;
  /** 그래프 칸의 폭(px, `graphWidth`). 0 이면 그래프 없음. */
  graphPx: number;
  palette: Palette;
  font: string;
  selected: ReadonlySet<string>;
  /** 강조할 줄의 색 번호. */
  hoverColor?: number;
  formatDate(seconds: number): string;
  /** 「잘린 글자」 캐시. 폭 · 글꼴이 바뀌면 부르는 쪽이 비운다. */
  fitCache: Map<string, string>;
}

/** 화면에 든 행의 범위: `[first, last)`. */
export function visibleRange(scrollTop: number, height: number, count: number): [number, number] {
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT));
  const last = Math.min(count, Math.ceil((scrollTop + height) / ROW_HEIGHT));
  return [first, Math.max(first, last)];
}

function fit(ctx: Ctx, cache: Map<string, string>, text: string, width: number): string {
  if (width <= 0) return "";
  const key = `${width}\u0000${text}`;
  const hit = cache.get(key);
  if (hit !== undefined) return hit;
  let out = text;
  if (ctx.measureText(text).width > width) {
    // 글자 수로 반을 갈라 맞는 길이를 찾는다.
    let lo = 0;
    let hi = text.length;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      if (ctx.measureText(`${text.slice(0, mid)}…`).width <= width) lo = mid;
      else hi = mid - 1;
    }
    out = lo === 0 ? "" : `${text.slice(0, lo)}…`;
  }
  if (cache.size > 5000) cache.clear();
  cache.set(key, out);
  return out;
}

function strokeEdge(
  ctx: Ctx,
  palette: Palette,
  color: number,
  hover: number | undefined,
  draw: () => void,
) {
  const strong = hover !== undefined && color === hover && color !== FOLDED_COLOR;
  ctx.strokeStyle = laneColor(palette, color);
  ctx.lineWidth = strong ? 3 : 1.5;
  ctx.globalAlpha = hover === undefined || strong ? 1 : 0.35;
  ctx.beginPath();
  draw();
  ctx.stroke();
}

function drawGraph(ctx: Ctx, lanes: RowLanes, top: number, palette: Palette, hover?: number) {
  const g = lanes.graph;
  const mid = top + ROW_HEIGHT / 2;
  const bottom = top + ROW_HEIGHT;
  const nodeX = laneX(g.lane);
  for (const [lane, color] of lanes.pass)
    strokeEdge(ctx, palette, color, hover, () => {
      ctx.moveTo(laneX(lane), top);
      ctx.lineTo(laneX(lane), bottom);
    });
  for (const [lane, color] of g.up)
    strokeEdge(ctx, palette, color, hover, () => {
      ctx.moveTo(laneX(lane), top);
      ctx.lineTo(nodeX, mid);
    });
  for (const [lane, color] of g.down)
    strokeEdge(ctx, palette, color, hover, () => {
      ctx.moveTo(nodeX, mid);
      ctx.lineTo(laneX(lane), bottom);
    });
  ctx.globalAlpha = hover === undefined || hover === g.color ? 1 : 0.35;
  ctx.fillStyle = laneColor(palette, g.color);
  ctx.strokeStyle = palette.node;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.arc(nodeX, mid, NODE_RADIUS, 0, Math.PI * 2);
  ctx.fill();
  ctx.stroke();
  ctx.globalAlpha = 1;
}

/** 라벨의 글자. 분리된 HEAD 는 `HEAD`, 지금 브랜치는 앞에 표시를 단다. */
function labelText(ref: LogRef): string {
  return ref.current ? `● ${ref.name}` : ref.name;
}

/** 보이는 행을 그리고, 그린 ref 라벨의 자리를 돌려준다. */
export function drawLog(input: DrawInput): LabelHit[] {
  const { ctx, width, height, scrollTop, rows, lanes, first, palette, selected } = input;
  ctx.fillStyle = palette.background;
  ctx.fillRect(0, 0, width, height);
  ctx.font = input.font;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  const gw = input.graphPx;
  const narrow = width < NARROW;
  const right = width - TEXT_PAD;
  const hashX = right - HASH_WIDTH;
  const dateX = narrow ? hashX : hashX - DATE_WIDTH;
  const authorX = narrow ? hashX : dateX - AUTHOR_WIDTH;
  const subjectX = gw + TEXT_PAD;
  const labels: LabelHit[] = [];
  for (let k = 0; k < lanes.length; k++) {
    const index = first + k;
    const row = rows[index];
    if (!row) break;
    const top = index * ROW_HEIGHT - scrollTop;
    const mid = top + ROW_HEIGHT / 2;
    if (selected.has(row.hash)) {
      ctx.fillStyle = palette.selected;
      ctx.fillRect(0, top, width, ROW_HEIGHT);
    }
    const rowLanes = lanes[k];
    if (rowLanes) {
      ctx.save();
      ctx.beginPath();
      ctx.rect(0, top, gw, ROW_HEIGHT);
      ctx.clip();
      drawGraph(ctx, rowLanes, top, palette, input.hoverColor);
      ctx.restore();
    }
    // ref 라벨: 제목 앞에, 제목 칸의 절반까지만.
    let x = subjectX;
    const labelLimit = subjectX + Math.max(0, (authorX - subjectX) / 2);
    for (const ref of row.refs) {
      const text = fit(ctx, input.fitCache, labelText(ref), labelLimit - x - LABEL_PAD * 2);
      if (text === "") break;
      const w = ctx.measureText(text).width + LABEL_PAD * 2;
      const strong = ref.current || ref.kind === "head";
      ctx.strokeStyle = strong ? palette.accent : palette.border;
      ctx.lineWidth = 1;
      ctx.strokeRect(x + 0.5, mid - LABEL_HEIGHT / 2 + 0.5, w - 1, LABEL_HEIGHT - 1);
      ctx.fillStyle =
        ref.kind === "tag" || ref.kind === "remote" ? palette.muted : palette.foreground;
      ctx.fillText(text, x + LABEL_PAD, mid);
      labels.push({ row: index, ref, x, width: w });
      x += w + LABEL_GAP;
    }
    ctx.fillStyle = palette.foreground;
    ctx.fillText(fit(ctx, input.fitCache, row.subject, authorX - TEXT_PAD - x), x, mid);
    ctx.fillStyle = palette.muted;
    if (!narrow) {
      ctx.fillText(fit(ctx, input.fitCache, row.authorName, AUTHOR_WIDTH - TEXT_PAD), authorX, mid);
      ctx.fillText(input.formatDate(row.authorTime), dateX, mid);
    }
    ctx.fillText(row.hash.slice(0, 8), hashX, mid);
  }
  return labels;
}
