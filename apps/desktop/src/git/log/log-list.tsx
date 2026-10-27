// Git 로그의 가운데 목록(docs/design/13-m2-plan.md M2-14 · 결정 M2-D2): 고정 행 높이 가상 스크롤 위의 canvas 하나. 스크롤 칸 안에
// 전체 높이(행 수 × 행 높이)의 빈 칸을 두고, 그 위에 화면 크기의 canvas 를 붙여 화면에 든 행만 그린다(graph-render.ts). 스크롤 ·
// 크기 · 행 · 선택 · 테마 · 화면 배율(창을 다른 배율의 모니터로 옮길 때)이 바뀌면 다음 프레임에 한 번 그린다(여러 번 바뀌어도 한 번).
//
// 마우스(10-ui.md 「Git」 마우스 표): 클릭 = 고르기, `⌘`(Win/Linux `Ctrl`) = 더하고 빼기, `⇧` = 범위, 더블클릭 = 커밋 상세 탭, 레인
// 호버 = 그 줄 강조, ref 라벨을 4 px 넘게 끌어 다른 행 · 라벨에 놓기 = 머지 · 리베이스 · 비교 메뉴(부르는 쪽이 띄운다). 우클릭은 부르는
// 쪽의 메뉴(ContextMenu)가 받고, 여기서는 그 행을 알려 준다. 키: ↑ ↓ PageUp PageDown Home End 로 고르고 `⇧` 를 함께 누르면 범위,
// Enter = 커밋 상세 탭, `⌘C` = 해시 복사.
//
// canvas 의 글자는 스크린 리더가 읽지 못한다 — 고른 커밋을 숨긴 live 영역에 한 줄로 알린다.

import type { LogRef, LogRow } from "@pitwall/rpc-client";
import {
  type KeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useKeymap } from "../../keymap/keymap-provider";
import {
  colorAtLane,
  type Ctx,
  drawLog,
  graphWidth,
  type LabelHit,
  laneAt,
  type Palette,
  ROW_HEIGHT,
  visibleRange,
} from "./graph-render";
import { type RowLanes, widestLane } from "./lanes";
import type { LogModel } from "./log-model";

/** 끌기로 보는 거리(px). */
const DRAG_THRESHOLD = 4;

/** 놓은 자리: 행(과 그 위의 라벨). */
export interface DropTarget {
  row: LogRow;
  ref?: LogRef;
}

export interface LogListProps {
  model: LogModel;
  palette: Palette;
  font: string;
  selected: ReadonlySet<string>;
  /** 고르기. `mode` 는 눌린 보조 키. */
  onSelect(index: number, mode: "single" | "toggle" | "range"): void;
  onOpen(row: LogRow): void;
  onCopy(): void;
  /** 우클릭한 행(과 라벨). 메뉴가 열리기 직전에 부른다. */
  onContextRow(row: LogRow, ref: LogRef | undefined): void;
  onDropLabel(source: LogRef, from: LogRow, target: DropTarget, at: { x: number; y: number }): void;
  /** 그린 뒤 부른다(성능 측정). */
  onDrawn?(): void;
  /** 이 행을 화면 안으로(고른 커밋을 부모로 옮길 때). 바뀔 때마다 한 번. */
  reveal?: { index: number; seq: number };
  label: string;
  describe(row: LogRow): string;
  formatDate(seconds: number): string;
}

/** 화면 배율(`devicePixelRatio`). 바뀌면 새 값으로 다시 그린다 — 지금 배율의 `resolution` 미디어 질의가 풀리는 때가 바뀐 때다. */
function useDevicePixelRatio(): number {
  const [dpr, setDpr] = useState(() => window.devicePixelRatio || 1);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(`(resolution: ${dpr}dppx)`);
    const changed = () => setDpr(window.devicePixelRatio || 1);
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  }, [dpr]);
  return dpr;
}

interface Drag {
  ref: LogRef;
  from: LogRow;
  x: number;
  y: number;
  active: boolean;
}

export function LogList(props: LogListProps) {
  const { model, palette, font, selected } = props;
  const mac = useKeymap().platform === "mac";
  const version = useSyncExternalStore(model.subscribe, model.getVersion);
  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [hoverColor, setHoverColor] = useState<number>();
  const dpr = useDevicePixelRatio();
  const [ghost, setGhost] = useState<{ text: string; x: number; y: number }>();
  const frame = useRef<number | undefined>(undefined);
  // 마지막으로 그린 것(누르기 판정).
  const drawn = useRef<{
    first: number;
    lanes: (RowLanes | null)[];
    graphPx: number;
    labels: LabelHit[];
  }>({ first: 0, lanes: [], graphPx: 0, labels: [] });
  const fitCache = useRef(new Map<string, string>());
  const drag = useRef<Drag | undefined>(undefined);
  const latest = useRef(props);
  latest.current = props;
  const hoverRef = useRef(hoverColor);
  hoverRef.current = hoverColor;

  const draw = useCallback(() => {
    frame.current = undefined;
    const canvas = canvasRef.current;
    const scroll = scrollRef.current;
    if (!canvas || !scroll) return;
    const p = latest.current;
    const ctx: Ctx | null = canvas.getContext("2d");
    if (!ctx) return;
    const width = scroll.clientWidth;
    const height = scroll.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      fitCache.current.clear();
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const scrollTop = scroll.scrollTop;
    const [first, last] = visibleRange(scrollTop, height, p.model.rows.length);
    const lanes = p.model.filtered ? [] : p.model.lanes.range(first, last);
    const rowsLanes = lanes.length > 0 ? lanes : Array.from({ length: last - first }, () => null);
    const graphPx = graphWidth(widestLane(lanes), width);
    const labels = drawLog({
      ctx,
      width,
      height,
      scrollTop,
      rows: p.model.rows,
      lanes: rowsLanes,
      first,
      graphPx,
      palette: p.palette,
      font: p.font,
      selected: p.selected,
      hoverColor: hoverRef.current,
      formatDate: p.formatDate,
      fitCache: fitCache.current,
    });
    drawn.current = { first, lanes: rowsLanes, graphPx, labels };
    p.onDrawn?.();
  }, []);

  const schedule = useCallback(() => {
    if (frame.current !== undefined) return;
    frame.current = requestAnimationFrame(draw);
  }, [draw]);

  useEffect(
    () => () => {
      if (frame.current !== undefined) cancelAnimationFrame(frame.current);
    },
    [],
  );

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const measure = () => setSize({ width: scroll.clientWidth, height: scroll.clientHeight });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(scroll);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    fitCache.current.clear();
  }, [font]);

  // 그릴 것이 바뀌면 다음 프레임에 그린다.
  useEffect(schedule, [schedule, version, size, palette, font, selected, hoverColor, dpr]);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!props.reveal || !scroll) return;
    const top = props.reveal.index * ROW_HEIGHT;
    if (top < scroll.scrollTop) scroll.scrollTop = top;
    else if (top + ROW_HEIGHT > scroll.scrollTop + scroll.clientHeight)
      scroll.scrollTop = top + ROW_HEIGHT - scroll.clientHeight;
  }, [props.reveal]);

  const pointAt = (clientX: number, clientY: number) => {
    const scroll = scrollRef.current!;
    const rect = scroll.getBoundingClientRect();
    const x = clientX - rect.left;
    const index = Math.floor((clientY - rect.top + scroll.scrollTop) / ROW_HEIGHT);
    const row = model.rows[index];
    const label = drawn.current.labels.find(
      (l) => l.row === index && x >= l.x && x < l.x + l.width,
    );
    return { x, index, row, label };
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const { index, row, label } = pointAt(e.clientX, e.clientY);
    if (!row) return;
    const toggle = mac ? e.metaKey : e.ctrlKey;
    props.onSelect(index, e.shiftKey ? "range" : toggle ? "toggle" : "single");
    if (label)
      drag.current = { ref: label.ref, from: row, x: e.clientX, y: e.clientY, active: false };
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (d) {
      if (!d.active && Math.hypot(e.clientX - d.x, e.clientY - d.y) > DRAG_THRESHOLD) {
        d.active = true;
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }
      if (d.active) {
        const rect = scrollRef.current!.getBoundingClientRect();
        setGhost({ text: d.ref.name, x: e.clientX - rect.left + 12, y: e.clientY - rect.top + 8 });
        return;
      }
    }
    const { x, index } = pointAt(e.clientX, e.clientY);
    const lanes = drawn.current.lanes[index - drawn.current.first];
    const lane = laneAt(x, drawn.current.graphPx);
    const color = lanes && lane >= 0 ? colorAtLane(lanes, lane) : undefined;
    if (color !== hoverRef.current) setHoverColor(color);
  };

  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = undefined;
    setGhost(undefined);
    if (!d?.active) return;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    const { row, label } = pointAt(e.clientX, e.clientY);
    if (
      !row ||
      (row.hash === d.from.hash &&
        (!label || (label.ref.name === d.ref.name && label.ref.kind === d.ref.kind)))
    )
      return;
    props.onDropLabel(d.ref, d.from, { row, ref: label?.ref }, { x: e.clientX, y: e.clientY });
  };

  const onContextMenu = (e: ReactMouseEvent<HTMLDivElement>) => {
    const { index, row, label } = pointAt(e.clientX, e.clientY);
    if (!row) {
      e.preventDefault();
      return;
    }
    if (!selected.has(row.hash)) props.onSelect(index, "single");
    props.onContextRow(row, label?.ref);
  };

  // 고른 것 중 맨 위 행(고른 수는 적다 — 행 전체를 훑지 않는다).
  let current = -1;
  for (const hash of selected) {
    const i = model.indexOf(hash);
    if (i >= 0 && (current < 0 || i < current)) current = i;
  }
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const count = model.rows.length;
    if (count === 0) return;
    const scroll = scrollRef.current!;
    const page = Math.max(1, Math.floor(scroll.clientHeight / ROW_HEIGHT) - 1);
    const mod = mac ? e.metaKey : e.ctrlKey;
    let next: number | undefined;
    const at = lastFocus.current ?? Math.max(0, current);
    if (e.key === "ArrowDown") next = Math.min(count - 1, at + 1);
    else if (e.key === "ArrowUp") next = Math.max(0, at - 1);
    else if (e.key === "PageDown") next = Math.min(count - 1, at + page);
    else if (e.key === "PageUp") next = Math.max(0, at - page);
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = count - 1;
    else if (e.key === "Enter" && current >= 0) {
      e.preventDefault();
      props.onOpen(model.rows[current]!);
      return;
    } else if (mod && e.key.toLowerCase() === "c") {
      e.preventDefault();
      props.onCopy();
      return;
    }
    if (next === undefined) return;
    e.preventDefault();
    lastFocus.current = next;
    props.onSelect(next, e.shiftKey ? "range" : "single");
    const top = next * ROW_HEIGHT;
    if (top < scroll.scrollTop) scroll.scrollTop = top;
    else if (top + ROW_HEIGHT > scroll.scrollTop + scroll.clientHeight)
      scroll.scrollTop = top + ROW_HEIGHT - scroll.clientHeight;
  };
  const lastFocus = useRef<number | undefined>(undefined);

  const selectedRow = current >= 0 ? model.rows[current] : undefined;
  return (
    <div className="relative h-full min-h-0 flex-1">
      <div
        ref={scrollRef}
        data-testid="git-log-list"
        role="grid"
        aria-label={props.label}
        aria-rowcount={model.rows.length}
        tabIndex={0}
        className="absolute inset-0 overflow-auto outline-none focus-visible:ring-1 focus-visible:ring-ui-accent"
        onScroll={schedule}
        onPointerDown={(e) => {
          lastFocus.current = undefined;
          onPointerDown(e);
        }}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={() => {
          drag.current = undefined;
          setGhost(undefined);
        }}
        onPointerLeave={() => {
          if (!drag.current?.active && hoverRef.current !== undefined) setHoverColor(undefined);
        }}
        onDoubleClick={(e) => {
          const { row } = pointAt(e.clientX, e.clientY);
          if (row) props.onOpen(row);
        }}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
      >
        {/* canvas 는 화면 크기로 위에 붙고, 빈 칸이 나머지 높이를 채워 스크롤 범위가 행 수 × 행 높이가 된다. */}
        <canvas ref={canvasRef} className="pointer-events-none sticky top-0 left-0 block" />
        <div
          style={{ height: Math.max(0, model.rows.length * ROW_HEIGHT - size.height), width: 1 }}
        />
      </div>
      {ghost && (
        <div
          className="pointer-events-none absolute rounded-sm border border-ui-accent bg-ui-panel px-1 text-[12px]"
          style={{ left: ghost.x, top: ghost.y }}
        >
          {ghost.text}
        </div>
      )}
      <div className="sr-only" aria-live="polite">
        {selectedRow ? props.describe(selectedRow) : ""}
      </div>
    </div>
  );
}
