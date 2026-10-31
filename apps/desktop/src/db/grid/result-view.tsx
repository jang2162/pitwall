// 결과 하나의 화면(M3-16): 필터 막대 · 그리드 · 셀 상세 패널 · 상태 줄. SQL 콘솔의 결과 탭(M3-15) · 테이블 데이터(M3-17) ·
// Redis 표(M3-18)가 이것(또는 그리드만 — glide-grid.tsx 의 `ResultGrid`)을 쓴다.
//
// - 상태 줄: 행 수(거르면 「보이는 / 받은」) · 시간 · 「상한에서 멈춤(N행)」(Host 가 그리드 보유 행 상한에서 커서를 버렸다 —
//   결정 M3-D7) · 바뀐 행 수 · 오류.
// - 복사: ⌘C 는 TSV(그리드의 copy 이벤트), 「다른 형식으로 복사」 는 막대 단추 · 우클릭 · ⌘⇧C(액션 `db.grid.copyAs` —
//   db-grid-feature.tsx). 결과 그림의 「⤓ 내보내기」 는 두지 않는다(F6-11 과 함께 이월 — 14-m3-plan §3).
// - 이 컴포넌트가 요구하는 공급자는 테마(`ThemeStoreProvider`)와 번역(`LocalizationProvider`)뿐이다 — Host 연결 · 알림은 쓰는
//   화면이 넘긴다(시험과 재사용이 가볍게).
// - **결과에 묶인 화면 상태는 한 곳(`ResultBody`, 결과 id `buffer.id` 를 키로)에 둔다**: 필터 칸의 글 · 열, 고르기 · 커서,
//   「다른 형식으로 복사」 메뉴, 그리드의 열 순서 · 너비 · 스크롤. 다른 결과를 넘기면 몸체가 새로 생겨 하나도 넘어오지 않는다
//   (M3-16 리뷰: 앞 결과의 필터가 다음 결과에 걸려 「0 / N」 으로 보였다). 정렬 · 필터 자체는 버퍼의 것이라 그 결과로 돌아가면
//   그대로 보인다. 결과와 무관한 것(셀 상세 패널을 열어 두었는가)만 바깥 `ResultView` 에 둔다 — 새 상태를 더할 때는 `ResultBody`
//   에 둔다.
// - 테이블 데이터(M3-17)가 쓰는 가산 이음매(넘기지 않으면 지금 그대로): `onActivate`(셀 더블클릭 · ↩ — 기본은 셀 상세),
//   `cellMark` · `onCellClick`(glide-grid.tsx), `localFilter: false`(보유 행 필터 막대를 숨긴다 — 서버의 WHERE 를 쓰는 화면),
//   `gridRef`(그리드 핸들 — 고르기 · 셀 사각형), `onCursor`(지금 칸), `keymapContext`(그리드 영역에 더할 키맵 컨텍스트 키 —
//   src/keymap/context.ts 는 가장 가까운 `data-keymap-context` 만 읽는다).
// - 「다른 형식으로 복사」 는 Host 의 클립보드 쓰기로 쓴다(clipboard.ts — 비동기 뒤의 `navigator.clipboard.writeText` 는 WebKit
//   이 거절할 수 있다).

import * as ContextMenu from "@radix-ui/react-context-menu";
import type { ObjectRef, RpcClient, SqlDialect } from "@pitwall/rpc-client";
import {
  type KeyboardEvent,
  type MutableRefObject,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import type { NotificationInput } from "../../notifications/notifications";
import { useTheme } from "../../theme/theme-switcher";
import { CellDetail } from "./cell-detail";
import { writeClipboardText } from "./clipboard";
import { COPY_FORMATS, type CopyFormat, CopyRefused, formatBlock } from "./copy";
import { type GridCellMark, ResultGrid, type ResultGridHandle } from "./glide-grid";
import { clearActiveGrid, type GridTarget, setActiveGrid } from "./grid-focus";
import { gridColors } from "./grid-theme";
import type { ResultBuffer } from "./result-buffer";
import { isEmptyPick, pickBlock } from "./selection";
import { type DbGridStrings, useDbGridStrings } from "./strings";
import { formatSize } from "./values";

export interface ResultViewProps {
  buffer: ResultBuffer;
  /** INSERT 로 복사할 때 쓰는 방언 · 테이블(모르면 `MY_TABLE`)과 Host. 없으면 INSERT 형식이 꺼진다. */
  insert?: { client: RpcClient; dialect: SqlDialect; table: ObjectRef | null };
  /** 복사 결과 · 거절을 알린다. 쓰는 화면이 `useNotify()` 를 넘긴다. */
  notify?: (input: NotificationInput) => void;
  /** 클립보드에 쓴다. 기본은 Host 의 클립보드 쓰기(clipboard.ts). */
  writeClipboard?: (text: string) => Promise<void>;
  /** 막대 오른쪽에 붙일 것(콘솔의 결과 탭 단추 등). */
  toolbar?: ReactNode;
  /** 셀 더블클릭 · ↩. 없으면 그 셀의 상세 패널을 연다. */
  onActivate?: (cell: { row: number; column: number }) => void;
  /** 편집 대기 표지(glide-grid.tsx `cellMark`). */
  cellMark?: (row: number, column: number) => GridCellMark | undefined;
  /** 셀 클릭(glide-grid.tsx `onCellClick`). */
  onCellClick?: (cell: { row: number; column: number }, modifiers: { mod: boolean }) => void;
  /** 보유한 행 위의 필터 막대를 두는가(기본 `true`). */
  localFilter?: boolean;
  /** 그리드 핸들을 받는다. */
  gridRef?: MutableRefObject<ResultGridHandle | null>;
  /** 지금 칸이 바뀌었다. */
  onCursor?: (cell: { row: number; column: number } | undefined) => void;
  /** 그리드 영역에 함께 둘 키맵 컨텍스트 키(`dbGridFocus` 에 더한다 — 가장 가까운 컨텍스트만 읽히므로). */
  keymapContext?: string;
}

function useVersion(buffer: ResultBuffer) {
  return useSyncExternalStore(
    useCallback((f) => buffer.subscribe(f), [buffer]),
    () => buffer.version,
  );
}

/** 버퍼의 값 하나만 구독한다 — 그 값이 바뀔 때만 다시 그린다(행이 붙을 때마다 화면 전체를 다시 그리지 않게). */
function useBufferValue<T>(buffer: ResultBuffer, read: (b: ResultBuffer) => T): T {
  return useSyncExternalStore(
    useCallback((f) => buffer.subscribe(f), [buffer]),
    () => read(buffer),
  );
}

export function StatusLine({ buffer, T }: { buffer: ResultBuffer; T: DbGridStrings }) {
  useVersion(buffer);
  const s = buffer.state;
  const parts: string[] = [];
  const filtered = buffer.filter !== undefined;
  if (s.kind === "streaming") parts.push(T.status.loading(buffer.loadedRows));
  else if (buffer.columns.length === 0 && s.kind === "done") {
    parts.push(s.rowsAffected === null ? T.status.noColumns : T.status.affected(s.rowsAffected));
  } else {
    parts.push(
      filtered
        ? T.status.filtered(buffer.rowCount, buffer.loadedRows)
        : T.status.rows(buffer.loadedRows),
    );
  }
  if (s.kind !== "streaming") parts.push(T.status.elapsed(s.elapsedMs));
  return (
    <div
      className="flex h-6 shrink-0 items-center gap-3 border-t border-ui-border bg-ui-statusbar px-2 text-[11px] text-ui-statusbar-foreground"
      data-testid="db-grid-status"
      data-state={s.kind}
    >
      <span>{parts.join(" · ")}</span>
      {s.kind === "done" && s.truncated && (
        <span className="font-semibold" title={T.status.cappedHint} data-testid="db-grid-capped">
          {T.status.capped(buffer.loadedRows, formatSize(buffer.heldBytes))}
        </span>
      )}
      {s.kind === "error" && (
        <span className="text-git-deleted" data-testid="db-grid-error">
          {s.cancelled ? T.status.cancelled : T.status.failed(s.message)}
        </span>
      )}
    </div>
  );
}

function FilterBar({ buffer, T }: { buffer: ResultBuffer; T: DbGridStrings }) {
  const columns = useBufferValue(buffer, (b) => b.columns);
  const [text, setText] = useState(buffer.filter?.text ?? "");
  const [column, setColumn] = useState<number | undefined>(buffer.filter?.column);
  // 필터는 칸이 멈춘 뒤에 건다(10만 행을 글자마다 거르지 않게).
  useEffect(() => {
    const t = setTimeout(() => buffer.setFilter(text ? { text, column } : undefined), 150);
    return () => clearTimeout(t);
  }, [buffer, text, column]);
  return (
    <>
      <input
        type="search"
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={T.filter.placeholder}
        aria-label={T.filter.placeholder}
        data-testid="db-grid-filter"
        className="h-6 w-56 rounded border border-ui-border bg-ui-background px-2 text-[12px]"
      />
      <select
        value={column ?? ""}
        onChange={(e) => setColumn(e.target.value === "" ? undefined : Number(e.target.value))}
        aria-label={T.filter.column}
        data-testid="db-grid-filter-column"
        className="h-6 rounded border border-ui-border bg-ui-background px-1 text-[12px]"
      >
        <option value="">{T.filter.allColumns}</option>
        {columns.map((c, i) => (
          <option key={i} value={i}>
            {c.name}
          </option>
        ))}
      </select>
    </>
  );
}

function CopyAsMenu({
  formats,
  T,
  onPick,
  onClose,
}: {
  formats: readonly CopyFormat[];
  T: DbGridStrings;
  onPick: (f: CopyFormat) => void;
  onClose: () => void;
}) {
  const first = useRef<HTMLButtonElement>(null);
  useEffect(() => first.current?.focus(), []);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button")];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length];
    next?.focus();
  };
  return (
    <div
      role="menu"
      aria-label={T.copy.copyAsTitle}
      data-testid="db-grid-copy-as"
      onKeyDown={onKeyDown}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) onClose();
      }}
      className="absolute top-7 right-2 z-20 min-w-[180px] rounded-md border border-ui-border bg-ui-panel py-1 text-[12px] shadow-lg"
    >
      {formats.map((f, i) => (
        <button
          key={f}
          ref={i === 0 ? first : undefined}
          type="button"
          role="menuitem"
          data-format={f}
          className="flex h-6 w-full items-center px-3 text-left outline-none hover:bg-ui-active focus:bg-ui-active"
          onClick={() => onPick(f)}
        >
          {T.copy.format[f]}
        </button>
      ))}
    </div>
  );
}

const MENU_ITEM =
  "flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active";

export function ResultView(props: ResultViewProps) {
  // 결과와 무관한 화면 상태만 여기 — 결과에 묶인 것은 `ResultBody`(결과 id 를 키로).
  const [detail, setDetail] = useState(false);
  return <ResultBody key={props.buffer.id} {...props} detail={detail} setDetail={setDetail} />;
}

function ResultBody({
  buffer,
  insert,
  notify,
  writeClipboard,
  toolbar,
  onActivate,
  cellMark,
  onCellClick,
  localFilter = true,
  gridRef,
  onCursor,
  keymapContext,
  detail,
  setDetail,
}: ResultViewProps & { detail: boolean; setDetail: (open: boolean) => void }) {
  const T = useDbGridStrings();
  const themeValue = useTheme((s) => s.theme);
  const appearance = useTheme((s) => s.appearance);
  const colors = useMemo(() => {
    const values = themeValue.variants[appearance];
    if (!values) throw new Error(`테마 ${themeValue.id} 에 ${appearance} 변형이 없다`);
    return gridColors(values);
  }, [themeValue, appearance]);
  const grid = useRef<ResultGridHandle>(null);
  const root = useRef<HTMLDivElement>(null);
  const [cursor, setCursor] = useState<{ row: number; column: number }>();
  const [copyAs, setCopyAs] = useState(false);
  const formats = useMemo(
    () => COPY_FORMATS.filter((f) => f !== "insert" || insert !== undefined),
    [insert],
  );

  const write = useCallback(
    (text: string) => (writeClipboard ?? writeClipboardText)(text),
    [writeClipboard],
  );

  const copy = useCallback(
    async (format: CopyFormat) => {
      const handle = grid.current;
      if (!handle) return;
      const pick = handle.pick();
      if (isEmptyPick(pick)) {
        notify?.({ level: "info", title: T.copy.nothing });
        return;
      }
      const block = pickBlock(buffer, handle.order(), pick);
      try {
        await write(await formatBlock(format, block, insert));
        notify?.({ level: "info", title: T.copy.copied(format, block.rows.length) });
      } catch (error) {
        if (error instanceof CopyRefused) {
          const title =
            error.reason === "truncated" && error.cell
              ? T.copy.truncated(error.cell.row + 1, block.columns[error.cell.column]?.name ?? "")
              : error.reason === "nul"
                ? T.copy.nul
                : error.reason === "tooLarge"
                  ? T.copy.tooLarge
                  : T.copy.unsupported;
          notify?.({ level: "warning", title });
          return;
        }
        notify?.({
          level: "error",
          title: T.copy.failed(error instanceof Error ? error.message : String(error)),
        });
      }
    },
    [buffer, insert, notify, write, T],
  );

  const target: GridTarget = useMemo(
    () => ({ root: () => root.current, openCopyAs: () => setCopyAs(true) }),
    [],
  );
  useEffect(() => () => clearActiveGrid(target), [target]);

  // 셀 상세가 열려 있을 때만 그 셀의 값을 구독한다(정렬 · 필터로 그 자리의 행이 바뀐다).
  const cursorValue = useBufferValue(buffer, (b) =>
    !detail || cursor === undefined ? undefined : b.row(cursor.row)?.[cursor.column],
  );
  const cursorColumn = useBufferValue(buffer, (b) =>
    cursor === undefined ? undefined : b.columns[cursor.column],
  );

  return (
    <div
      ref={root}
      className="relative flex min-h-0 flex-1 flex-col"
      data-keymap-context={keymapContext ? `dbGridFocus ${keymapContext}` : "dbGridFocus"}
      onFocus={() => setActiveGrid(target)}
    >
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-ui-border px-2">
        {localFilter && <FilterBar buffer={buffer} T={T} />}
        <span className="flex-1" />
        {toolbar}
        <button
          type="button"
          className="h-6 rounded px-2 text-[12px] hover:bg-ui-hover"
          aria-haspopup="menu"
          aria-expanded={copyAs}
          data-testid="db-grid-copy-as-button"
          onClick={() => setCopyAs(!copyAs)}
        >
          {T.copy.copyAs}
        </button>
        <button
          type="button"
          className="h-6 rounded px-2 text-[12px] hover:bg-ui-hover"
          aria-pressed={detail}
          data-testid="db-grid-detail-toggle"
          onClick={() => setDetail(!detail)}
        >
          {detail ? T.detail.close : T.detail.show}
        </button>
        {copyAs && (
          <CopyAsMenu
            formats={formats}
            T={T}
            onClose={() => {
              setCopyAs(false);
              grid.current?.focus();
            }}
            onPick={(f) => {
              setCopyAs(false);
              grid.current?.focus();
              void copy(f);
            }}
          />
        )}
      </div>
      <div className="flex min-h-0 flex-1">
        <ContextMenu.Root>
          <ContextMenu.Trigger asChild>
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <ResultGrid
                ref={(handle) => {
                  grid.current = handle;
                  if (gridRef) gridRef.current = handle;
                }}
                buffer={buffer}
                colors={colors}
                cellMark={cellMark}
                onCellClick={onCellClick}
                onCursor={(cell) => {
                  setCursor(cell);
                  onCursor?.(cell);
                }}
                onActivate={(cell) => {
                  setCursor(cell);
                  if (onActivate) onActivate(cell);
                  else setDetail(true);
                }}
              />
            </div>
          </ContextMenu.Trigger>
          <ContextMenu.Portal>
            <ContextMenu.Content
              data-testid="db-grid-menu"
              className="min-w-[200px] rounded-md border border-ui-border bg-ui-panel py-1 text-[12px] text-ui-foreground shadow-lg"
            >
              <ContextMenu.Item className={MENU_ITEM} onSelect={() => void copy("tsv")}>
                {T.copy.copy}
              </ContextMenu.Item>
              <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
              {formats.map((f) => (
                <ContextMenu.Item key={f} className={MENU_ITEM} onSelect={() => void copy(f)}>
                  {T.copy.copyAsTitle}: {T.copy.format[f]}
                </ContextMenu.Item>
              ))}
              <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
              <ContextMenu.Item className={MENU_ITEM} onSelect={() => setDetail(true)}>
                {T.detail.show}
              </ContextMenu.Item>
            </ContextMenu.Content>
          </ContextMenu.Portal>
        </ContextMenu.Root>
        {detail && (
          <aside
            className="flex w-80 min-w-0 shrink-0 flex-col border-l border-ui-border"
            aria-label={T.detail.title}
          >
            <CellDetail value={cursorValue} column={cursorColumn} T={T} />
          </aside>
        )}
      </div>
      <StatusLine buffer={buffer} T={T} />
    </div>
  );
}
