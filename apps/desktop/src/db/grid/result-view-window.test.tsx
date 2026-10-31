// @vitest-environment happy-dom
// 결과 화면(M3-16)을 창에서 본다. Glide 는 canvas 라 happy-dom 에서 그림을 볼 수 없으므로 감싸개가 넘기는 것(셀 · 테마 · 콜백)을
// 기록하는 가짜로 바꾼다 — 실제 canvas 의 그림 · 성능은 헤드리스 WebKit 하네스(perf/result-grid)가 본다.
//
// 본다: 상한 + 1 행이면 「상한에서 멈춤」, 라이트 ↔ 다크 전환에 새 색으로 다시 칠함, DB 값은 늘 글자 칸(HTML 칸 · 오버레이
// 편집기 없음)이고 셀 상세도 글자로만 그림, 머리 클릭 정렬 · 열 순서 · 너비, ⌘C 의 TSV · 다른 형식으로 복사(CSV · INSERT),
// ⌘⇧C 액션이 포커스가 든 그리드에만 산다, 결과를 바꾸면 결과에 묶인 화면 상태가 하나도 넘어오지 않는다, NULL 은 색 + 기울임 +
// 접근성 글로 갈린다, 「다른 형식으로 복사」 는 Host 의 클립보드 쓰기로 간다(웹뷰의 `navigator.clipboard` 를 쓰지 않는다).

import { LocalizationProvider } from "@fluent/react";
import type { DbQueryEvent, DbValue, RpcClient } from "@pitwall/rpc-client";
import { forwardRef, useImperativeHandle, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { bundledLocalization, bundledTranslator } from "../../i18n/testing";
import { act, endTest, it } from "../../test-support/ended-test";
import { createThemeStore, type ThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { gridActions } from "./db-grid-feature";
import { focusedGrid } from "./grid-focus";
import { QueryRun } from "./query-stream";
import type { ResultBuffer } from "./result-buffer";
import { ResultGrid } from "./glide-grid";
import { gridColors } from "./grid-theme";
import { ResultView, type ResultViewProps } from "./result-view";
import { dbGridStrings } from "./strings";
import { formatSize } from "./values";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

interface FakeProps {
  columns: { id: string; title: string; width: number }[];
  rows: number;
  theme: Record<string, unknown>;
  getCellContent: (cell: [number, number]) => {
    kind: string;
    data?: string;
    displayData?: string;
    allowOverlay: boolean;
    readonly?: boolean;
    contentAlign?: string;
    themeOverride?: Record<string, string>;
  };
  onHeaderClicked: (
    col: number,
    e: { metaKey: boolean; ctrlKey: boolean; preventDefault: () => void },
  ) => void;
  onColumnMoved: (from: number, to: number) => void;
  onColumnResize: (column: { id: string }, size: number) => void;
  onGridSelectionChange: (s: unknown) => void;
  onCellActivated: (cell: [number, number]) => void;
  keybindings: Record<string, boolean>;
  gridSelection: { current?: unknown };
  drawHeader: (args: Record<string, unknown>) => void;
  /** 가짜 편집기가 몇 번째로 붙었는가 — 바뀌면 Glide 안의 상태(스크롤 등)는 새로 시작한다. */
  mount: number;
}

const glide = vi.hoisted(() => ({ props: [] as FakeProps[], mounts: 0 }));
const host = vi.hoisted(() => ({ invoke: vi.fn(async (_cmd: string, _args?: unknown) => {}) }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: host.invoke }));

vi.mock("@glideapps/glide-data-grid", () => ({
  CompactSelection: { empty: () => ({ toArray: () => [] }) },
  GridCellKind: { Text: "text", Loading: "loading" },
  DataEditor: forwardRef(function FakeEditor(props: Omit<FakeProps, "mount">, ref) {
    const [mount] = useState(() => ++glide.mounts);
    glide.props.push({ ...props, mount });
    useImperativeHandle(ref, () => ({ focus: () => {} }));
    return <button type="button" data-testid="fake-glide" />;
  }),
}));

const T = dbGridStrings(bundledTranslator("ko"));
const last = () => glide.props[glide.props.length - 1]!;
const DARK = parseTheme(defaultTheme).variants.dark!;
const LIGHT = parseTheme(defaultTheme).variants.light!;

let root: Root | undefined;
let element: HTMLDivElement;
let theme: ThemeStore;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  glide.props.length = 0;
  element = document.createElement("div");
  document.body.append(element);
  theme = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  element.remove();
  theme.dispose();
});

/** 문장 하나의 스트림을 결과로 옮긴다(Host 없이 항목을 그대로). */
function run(events: DbQueryEvent[]): ResultBuffer {
  const q = new QueryRun();
  for (const e of events) q.apply(e);
  return q.results[0]!.buffer;
}

const STARTED: DbQueryEvent[] = [
  { kind: "accepted", executionId: "e1", statements: 1 },
  { kind: "started", index: 0, range: { from: 0, to: 8 }, grade: "read" },
  {
    kind: "columns",
    index: 0,
    columns: [
      { name: "id", dbType: "int8", nullable: false },
      { name: "body", dbType: "text", nullable: true },
    ],
  },
];

const rows = (n: number, from = 0): DbValue[][] =>
  Array.from({ length: n }, (_, i) => [{ int: from + i }, { text: `row ${from + i}` }]);

async function mount(props: ResultViewProps) {
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={theme}>
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <ResultView {...props} />
        </LocalizationProvider>
      </ThemeStoreProvider>,
    ),
  );
  // 버퍼 알림은 다음 프레임에 온다.
  await act(async () => new Promise((r) => requestAnimationFrame(() => r(undefined))));
}

const byTestId = (id: string) => element.querySelector<HTMLElement>(`[data-testid="${id}"]`);

function select(range: { x: number; y: number; width: number; height: number }) {
  last().onGridSelectionChange({
    current: { cell: [range.x, range.y], range, rangeStack: [] },
    rows: { toArray: () => [] },
    columns: { toArray: () => [] },
  });
}

describe("status line", () => {
  it("says the grid stopped at the limit when the Host cut the cursor (limit + 1 rows)", async () => {
    const buffer = run([
      ...STARTED,
      { kind: "rows", index: 0, rows: rows(500) },
      { kind: "rows", index: 0, rows: rows(500, 500) },
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 42, truncated: true },
    ]);
    await mount({ buffer });
    const capped = byTestId("db-grid-capped");
    expect(capped?.textContent).toBe(T.status.capped(1000, formatSize(buffer.heldBytes)));
    // 행 상한이 먼저 닿았다: 1000행 · 받은 크기는 작다.
    expect(buffer.heldBytes).toBeLessThan(64 * 1024);
    expect(byTestId("db-grid-status")?.textContent).toContain(T.status.rows(1000));
    expect(byTestId("db-grid-status")?.textContent).toContain(T.status.elapsed(42));
  });

  it("says how many rows and how many bytes it held when the byte limit came first", async () => {
    const cell = "x".repeat(1_000_000);
    const buffer = run([
      ...STARTED,
      ...Array.from({ length: 5 }, (_, i): DbQueryEvent => ({
        kind: "rows",
        index: 0,
        rows: [[{ int: i }, { text: cell }]],
      })),
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 7, truncated: true },
    ]);
    await mount({ buffer });
    expect(buffer.heldBytes).toBe(5 * (8 + 1_000_000));
    expect(byTestId("db-grid-capped")?.textContent).toBe(T.status.capped(5, "4.8 MiB"));
  });

  it("does not claim a limit when the rows ended on their own, and shows errors", async () => {
    const buffer = run([
      ...STARTED,
      { kind: "rows", index: 0, rows: rows(3) },
      {
        kind: "error",
        index: 0,
        elapsedMs: 5,
        error: {
          kind: "query",
          message: 'relation "t" does not exist',
          code: null,
          rolledBack: null,
        } as never,
      },
    ]);
    await mount({ buffer });
    expect(byTestId("db-grid-capped")).toBeNull();
    expect(byTestId("db-grid-error")?.textContent).toBe(
      T.status.failed('relation "t" does not exist'),
    );
  });
});

describe("grid", () => {
  it("repaints with the new grid.* colours when the appearance switches", async () => {
    const buffer = run([...STARTED, { kind: "rows", index: 0, rows: rows(2) }]);
    await mount({ buffer });
    expect(last().theme.bgCell).toBe(LIGHT["grid.background"]);
    expect(last().theme.textDark).toBe(LIGHT["grid.foreground"]);
    expect(last().theme.textHeader).toBe(LIGHT["grid.headerForeground"]);
    await act(async () => theme.getState().setPreference("dark"));
    expect(last().theme.bgCell).toBe(DARK["grid.background"]);
    expect(last().theme.textDark).toBe(DARK["grid.foreground"]);
    expect(last().theme.borderColor).toBe(DARK["grid.border"]);
    expect(DARK["grid.background"]).not.toBe(LIGHT["grid.background"]);
    // NULL 의 흐린 색도 외관을 따른다.
    const nullBuffer = run([...STARTED, { kind: "rows", index: 0, rows: [["null", "null"]] }]);
    await act(async () => root!.unmount());
    await mount({ buffer: nullBuffer });
    expect(last().getCellContent([0, 0]).themeOverride?.textDark).toBe(DARK["grid.null"]);
  });

  it("tells NULL apart from the text NULL by shape and by what a screen reader reads, not by colour alone", async () => {
    const buffer = run([
      ...STARTED,
      { kind: "rows", index: 0, rows: [["null", { text: "NULL" }]] },
    ]);
    await mount({ buffer });
    const nullCell = last().getCellContent([0, 0]);
    const textCell = last().getCellContent([1, 0]);
    expect(nullCell.displayData).toBe("NULL");
    expect(textCell.displayData).toBe("NULL");
    // 모양: NULL 은 기울임, 글자 「NULL」 은 보통 글꼴.
    expect(nullCell.themeOverride?.baseFontStyle).toMatch(/^italic /);
    expect(textCell.themeOverride).toBeUndefined();
    // 접근성 표는 `data` 를 읽는다.
    expect(nullCell.data).toBe(T.nullValue);
    expect(textCell.data).toBe("NULL");
    expect(T.nullValue).not.toBe("NULL");
  });

  it("draws a selected column header in grid.background over grid.accent", async () => {
    const buffer = run([...STARTED, { kind: "rows", index: 0, rows: rows(1) }]);
    await mount({ buffer });
    const paint = (isSelected: boolean) => {
      const fills: string[] = [];
      const ctx = {
        save() {},
        restore() {},
        beginPath() {},
        rect() {},
        clip() {},
        measureText: () => ({ width: 10 }),
        fillText(this: { fillStyle: string }) {
          fills.push(this.fillStyle);
        },
        fillStyle: "",
        font: "",
        textAlign: "",
        textBaseline: "",
      };
      last().drawHeader({
        ctx,
        column: last().columns[0],
        rect: { x: 0, y: 0, width: 100, height: 28 },
        theme: { headerFontStyle: "600 12px", baseFontStyle: "12px", fontFamily: "x" },
        isSelected,
      });
      return fills;
    };
    expect(paint(false)).toEqual([LIGHT["grid.headerForeground"], LIGHT["grid.headerType"]]);
    expect(paint(true)).toEqual([LIGHT["grid.background"], LIGHT["grid.background"]]);
  });

  it("draws every DB value as plain text cells without the overlay editor", async () => {
    const hostile = '<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>';
    const values: DbValue[] = [
      "null",
      { bool: true },
      { int: "9007199254740993" },
      { float: 1.5 },
      { decimal: "0.10" },
      { text: hostile },
      { text: "[link](javascript:alert(1))" },
      { bytes: { hex: "0x3c696d67", size: 4, truncated: false } },
      { json: `{"html":"${hostile.replaceAll('"', '\\"')}"}` },
      { datetime: "2024-01-01 00:00:00" },
      { uuid: "6f1e…" },
      { array: [{ text: hostile }] },
      { truncated: { of: "text", prefix: hostile, size: 9e6, typeName: null } },
      { other: { text: hostile, typeName: "tsvector" } },
    ];
    const buffer = run([
      ...STARTED,
      { kind: "rows", index: 0, rows: values.map((v) => [{ int: 1 }, v]) },
    ]);
    await mount({ buffer });
    expect(last().keybindings.copy).toBe(false);
    for (let r = 0; r < values.length; r++) {
      const cell = last().getCellContent([1, r]);
      expect(cell.kind, JSON.stringify(values[r])).toBe("text");
      expect(cell.allowOverlay).toBe(false);
      expect(cell.readonly).toBe(true);
    }
    expect(last().getCellContent([1, 5]).data).toBe(hostile);
    // 숫자는 오른쪽 정렬(문자열로 온 큰 정수 · decimal 포함), 글은 왼쪽.
    expect(last().getCellContent([1, 2]).contentAlign).toBe("right");
    expect(last().getCellContent([1, 4]).contentAlign).toBe("right");
    expect(last().getCellContent([1, 5]).contentAlign).toBe("left");
    // 셀 상세도 글자로만 그린다.
    for (const r of [5, 8, 11, 12, 13]) {
      await act(async () => last().onCellActivated([1, r]));
      const detail = byTestId("db-cell-detail");
      expect(detail?.textContent).toContain("<img src=x");
    }
    expect(document.querySelector("img, script")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("sorts on header click, moves and resizes columns over the held rows", async () => {
    const buffer = run([
      ...STARTED,
      {
        kind: "rows",
        index: 0,
        rows: [
          [{ int: 2 }, { text: "b" }],
          [{ int: 10 }, { text: "a" }],
          [{ int: 1 }, "null"],
        ],
      },
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 1, truncated: false },
    ]);
    await mount({ buffer });
    const ids = () => [0, 1, 2].map((r) => last().getCellContent([0, r]).data);
    expect(ids()).toEqual(["2", "10", "1"]);
    const click = { metaKey: false, ctrlKey: false, preventDefault: () => {} };
    await act(async () => last().onHeaderClicked(0, click));
    expect(ids()).toEqual(["1", "2", "10"]);
    await act(async () => last().onHeaderClicked(0, click));
    expect(ids()).toEqual(["10", "2", "1"]);
    await act(async () => last().onHeaderClicked(0, click));
    expect(ids()).toEqual(["2", "10", "1"]);
    // ⌘+클릭은 열 고르기 — 정렬하지 않는다.
    await act(async () => last().onHeaderClicked(0, { ...click, metaKey: true }));
    expect(buffer.sort).toBeUndefined();
    await act(async () => last().onColumnMoved(0, 1));
    expect(last().columns.map((c) => c.title)).toEqual(["body (text)", "id (int8)"]);
    expect(last().getCellContent([0, 0]).data).toBe("b");
    await act(async () => last().onColumnResize({ id: "0" }, 222));
    expect(last().columns.find((c) => c.id === "0")?.width).toBe(222);
  });
});

describe("switching results", () => {
  it("drops every result-bound screen state when another result comes in: filter, sort, selection, cursor, detail, scroll, column layout", async () => {
    const three: DbQueryEvent[] = [
      { kind: "accepted", executionId: "a", statements: 1 },
      { kind: "started", index: 0, range: { from: 0, to: 8 }, grade: "read" },
      {
        kind: "columns",
        index: 0,
        columns: [
          { name: "x", dbType: "int4", nullable: false },
          { name: "y", dbType: "text", nullable: true },
          { name: "z", dbType: "text", nullable: true },
        ],
      },
      {
        kind: "rows",
        index: 0,
        rows: [
          [{ int: 1 }, { text: "a1-1" }, { text: "a2-1" }],
          [{ int: 2 }, { text: "a1-2" }, { text: "a2-2" }],
        ],
      },
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 1, truncated: false },
    ];
    const a = run(three);
    const b = run([
      { kind: "accepted", executionId: "b", statements: 1 },
      { kind: "started", index: 0, range: { from: 0, to: 8 }, grade: "read" },
      { kind: "columns", index: 0, columns: [{ name: "only", dbType: "text", nullable: true }] },
      { kind: "rows", index: 0, rows: [1, 2, 3, 4, 5].map((i) => [{ text: `b${i}` }]) },
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 1, truncated: false },
    ]);
    const view = (buffer: ResultBuffer) => (
      <ThemeStoreProvider store={theme}>
        <LocalizationProvider l10n={bundledLocalization("ko")}>
          <ResultView buffer={buffer} />
        </LocalizationProvider>
      </ThemeStoreProvider>
    );
    const filter = () => byTestId("db-grid-filter") as HTMLInputElement;
    const filterColumn = () => byTestId("db-grid-filter-column") as HTMLSelectElement;
    const settle = () => act(async () => new Promise((r) => setTimeout(r, 170)));
    await mount({ buffer: a });
    // A 에서: 필터(열 z, 「a2-1」), 정렬, 열 순서 · 너비, 고르기 · 커서, 셀 상세.
    await act(async () => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(filter(), "a2-1");
      filter().dispatchEvent(new Event("input", { bubbles: true }));
      filterColumn().value = "2";
      filterColumn().dispatchEvent(new Event("change", { bubbles: true }));
    });
    await settle();
    expect(a.filter).toEqual({ text: "a2-1", column: 2 });
    expect(a.rowCount).toBe(1);
    const click = { metaKey: false, ctrlKey: false, preventDefault: () => {} };
    await act(async () => last().onHeaderClicked(0, click));
    expect(a.sort).toEqual({ column: 0, direction: "asc" });
    await act(async () => last().onColumnMoved(2, 0));
    await act(async () => last().onColumnResize({ id: "2" }, 333));
    await act(async () => select({ x: 0, y: 0, width: 2, height: 1 }));
    await act(async () => last().onCellActivated([0, 0]));
    expect(byTestId("db-cell-detail")?.textContent).toContain("a2-1");
    const mountA = last().mount;

    await act(async () => root!.render(view(b)));
    await settle();
    // B: 앞 결과의 필터 칸 · 열 · 정렬 · 고르기 · 커서 · 레이아웃 · Glide 안의 상태가 하나도 없다.
    expect(filter().value).toBe("");
    expect(filterColumn().value).toBe("");
    expect(b.filter).toBeUndefined();
    expect(b.sort).toBeUndefined();
    expect(b.rowCount).toBe(5);
    expect(last().rows).toBe(5);
    expect(byTestId("db-grid-status")?.textContent).toContain(T.status.rows(5));
    expect(last().gridSelection.current).toBeUndefined();
    expect(last().columns.map((c) => c.id)).toEqual(["0"]);
    expect(last().columns[0]!.width).not.toBe(333);
    expect(last().mount).not.toBe(mountA);
    // 셀 상세 패널은 열린 채(결과와 무관한 화면 상태)지만 고른 셀이 없다.
    expect(byTestId("db-cell-detail")?.textContent).toBe(T.detail.none);
    // A 의 필터는 B 가 덮어쓰지 않았다 — 돌아가면 A 의 것 그대로 보인다.
    expect(a.filter).toEqual({ text: "a2-1", column: 2 });
    await act(async () => root!.render(view(a)));
    await settle();
    expect(filter().value).toBe("a2-1");
    expect(filterColumn().value).toBe("2");
    expect(a.rowCount).toBe(1);
  });

  it("gives the grid alone (M3-17 · M3-18 reuse it) a fresh body per result: Glide state, selection and layout", async () => {
    const a = run([...STARTED, { kind: "rows", index: 0, rows: rows(3) }]);
    const b = run([...STARTED, { kind: "rows", index: 0, rows: rows(4) }]);
    const grid = (buffer: ResultBuffer) => (
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <ResultGrid buffer={buffer} colors={gridColors(LIGHT)} />
      </LocalizationProvider>
    );
    root = createRoot(element);
    await act(async () => root!.render(grid(a)));
    await act(async () => last().onColumnMoved(1, 0));
    await act(async () => select({ x: 0, y: 0, width: 1, height: 1 }));
    expect(last().gridSelection.current).toBeDefined();
    const mountA = last().mount;
    // 같은 열 꼴의 다른 결과 — 열 목록으로는 갈리지 않는다(결과 id 로 갈린다).
    await act(async () => root!.render(grid(b)));
    expect(last().mount).not.toBe(mountA);
    expect(last().gridSelection.current).toBeUndefined();
    expect(last().columns.map((c) => c.id)).toEqual(["0", "1"]);
  });

  it("starts the next result with its own column order before its columns arrive", async () => {
    const first = run([
      ...STARTED,
      { kind: "rows", index: 0, rows: rows(2) },
      { kind: "done", index: 0, rowsAffected: null, elapsedMs: 1, truncated: false },
    ]);
    await mount({ buffer: first });
    await act(async () => last().onColumnMoved(1, 0));
    await act(async () => last().onColumnResize({ id: "1" }, 300));
    expect(last().columns.map((c) => c.title)).toEqual(["body (text)", "id (int8)"]);
    // 다음 실행: `started` 만 받은 결과(열이 아직 없다)로 바꾼다 — 옛 순서로 새 열을 읽으면 렌더가 던져 화면이 사라진다.
    const q = new QueryRun();
    q.apply({ kind: "accepted", executionId: "e2", statements: 1 });
    q.apply({ kind: "started", index: 0, range: { from: 0, to: 8 }, grade: "read" });
    const next = q.results[0]!.buffer;
    await act(async () =>
      root!.render(
        <ThemeStoreProvider store={theme}>
          <LocalizationProvider l10n={bundledLocalization("ko")}>
            <ResultView buffer={next} />
          </LocalizationProvider>
        </ThemeStoreProvider>,
      ),
    );
    expect(byTestId("db-result-grid")).not.toBeNull();
    expect(last().columns).toEqual([]);
    await act(async () => {
      q.apply({
        kind: "columns",
        index: 0,
        columns: [{ name: "only", dbType: "text", nullable: true }],
      });
      q.apply({ kind: "rows", index: 0, rows: [[{ text: "v" }]] });
      await new Promise((r) => requestAnimationFrame(() => r(undefined)));
    });
    expect(last().columns.map((c) => [c.title, c.width])).toEqual([
      ["only (text)", expect.any(Number)],
    ]);
    expect(last().columns[0]!.width).not.toBe(300);
    expect(last().getCellContent([0, 0]).data).toBe("v");
  });
});

describe("copy", () => {
  it("writes TSV of the selected range on the copy event", async () => {
    const buffer = run([
      ...STARTED,
      {
        kind: "rows",
        index: 0,
        rows: [
          [{ int: 1 }, { text: "a\tb" }],
          [{ int: 2 }, "null"],
        ],
      },
    ]);
    await mount({ buffer });
    await act(async () => select({ x: 0, y: 0, width: 2, height: 2 }));
    const data = new Map<string, string>();
    const event = new Event("copy", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { setData: (t: string, v: string) => void };
    };
    event.clipboardData = { setData: (t, v) => data.set(t, v) };
    await act(async () => byTestId("fake-glide")!.dispatchEvent(event));
    expect(event.defaultPrevented).toBe(true);
    expect(data.get("text/plain")).toBe('1\t"a\tb"\n2\t');
  });

  it("copies as CSV and as INSERT through the menu, and refuses truncated cells for INSERT", async () => {
    const buffer = run([
      ...STARTED,
      {
        kind: "rows",
        index: 0,
        rows: [
          [{ int: 1 }, { text: 'x,"y"' }],
          [{ int: 2 }, { truncated: { of: "text", prefix: "p", size: 9e6, typeName: null } }],
        ],
      },
    ]);
    const written: string[] = [];
    const notes: string[] = [];
    const call = vi.fn(async () => ({ text: "INSERT INTO MY_TABLE …;\n" }));
    await mount({
      buffer,
      insert: { client: { call } as unknown as RpcClient, dialect: "postgres", table: null },
      notify: (n) => notes.push(n.title),
      writeClipboard: async (t) => void written.push(t),
    });
    const pickFormat = async (format: string) => {
      await act(async () => byTestId("db-grid-copy-as-button")!.click());
      const item = element.querySelector<HTMLButtonElement>(
        `[data-testid="db-grid-copy-as"] [data-format="${format}"]`,
      );
      await act(async () => item!.click());
    };
    await act(async () => select({ x: 0, y: 0, width: 2, height: 1 }));
    await pickFormat("csv");
    expect(written.pop()).toBe('id,body\r\n1,"x,""y"""\r\n');
    expect(notes.pop()).toBe(T.copy.copied("csv", 1));
    await pickFormat("insert");
    expect(written.pop()).toBe("INSERT INTO MY_TABLE …;\n");
    expect(call).toHaveBeenCalledWith("db.sql.insertText", {
      dialect: "postgres",
      table: null,
      columns: ["id", "body"],
      rows: [[{ int: 1 }, { text: 'x,"y"' }]],
    });
    await act(async () => select({ x: 0, y: 0, width: 2, height: 2 }));
    await pickFormat("insert");
    expect(call).toHaveBeenCalledTimes(1);
    expect(written).toEqual([]);
    expect(notes.pop()).toBe(T.copy.truncated(2, "body"));
  });

  it("writes copy-as text through the Host clipboard, also after the INSERT round trip, never through the webview clipboard", async () => {
    const buffer = run([...STARTED, { kind: "rows", index: 0, rows: rows(2) }]);
    const webview = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: webview, write: webview },
    });
    host.invoke.mockClear();
    let answer: (v: { text: string }) => void = () => {};
    const call = vi.fn(
      () =>
        new Promise<{ text: string }>((r) => {
          answer = r;
        }),
    );
    await mount({
      buffer,
      insert: { client: { call } as unknown as RpcClient, dialect: "postgres", table: null },
    });
    await act(async () => select({ x: 0, y: 0, width: 2, height: 1 }));
    const pick = async (format: string) => {
      await act(async () => byTestId("db-grid-copy-as-button")!.click());
      await act(async () =>
        element
          .querySelector<HTMLButtonElement>(
            `[data-testid="db-grid-copy-as"] [data-format="${format}"]`,
          )!
          .click(),
      );
    };
    await pick("csv");
    expect(host.invoke).toHaveBeenLastCalledWith("write_clipboard", {
      text: "id,body\r\n0,row 0\r\n",
    });
    // INSERT: Host 가 글을 돌려준 뒤(사용자 동작이 끝난 뒤)에도 Host 의 클립보드로 쓴다.
    await pick("insert");
    expect(host.invoke).toHaveBeenCalledTimes(1);
    await act(async () => answer({ text: "INSERT …;\n" }));
    expect(host.invoke).toHaveBeenLastCalledWith("write_clipboard", { text: "INSERT …;\n" });
    expect(webview).not.toHaveBeenCalled();
    Reflect.deleteProperty(navigator, "clipboard");
  });

  it("offers no INSERT format without a Host and dialect", async () => {
    const buffer = run([...STARTED, { kind: "rows", index: 0, rows: rows(1) }]);
    await mount({ buffer });
    await act(async () => byTestId("db-grid-copy-as-button")!.click());
    const formats = [...element.querySelectorAll("[data-format]")].map((e) =>
      e.getAttribute("data-format"),
    );
    expect(formats).toEqual(["csv", "json", "tsv"]);
  });

  it("lets the copy-as action run only while focus is inside a grid", async () => {
    const buffer = run([...STARTED, { kind: "rows", index: 0, rows: rows(1) }]);
    await mount({ buffer });
    const [action] = gridActions(T);
    const outside = document.createElement("button");
    document.body.append(outside);
    await act(async () => byTestId("fake-glide")!.focus());
    expect(focusedGrid()).toBeDefined();
    expect(action!.enabled!()).toBe(true);
    // 그리드를 거쳐 간 포커스가 밖으로 나가면 액션이 꺼진다(마지막 그리드를 기억해도).
    outside.focus();
    expect(focusedGrid()).toBeUndefined();
    expect(action!.enabled!()).toBe(false);
    await act(async () => byTestId("fake-glide")!.focus());
    await act(async () => void action!.run());
    expect(byTestId("db-grid-copy-as")).not.toBeNull();
    outside.remove();
  });
});
