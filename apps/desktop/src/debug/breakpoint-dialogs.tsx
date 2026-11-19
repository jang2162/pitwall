// 브레이크포인트 편집 팝업(거터 우클릭)과 목록(⌘⇧F8 — `debug.viewBreakpoints`), 17-m6-plan M6-18 ①.
//
// - 편집: 그 줄의 켬 · 조건 · 로그 메시지 · hit count 와 지우기. 고친 것은 [닫기] · Enter 로 문서의 브레이크포인트에 넣는다(Esc · 바깥은
//   버림). 그 문서에 미저장이 없으면 곧바로 Host 에, 있으면 저장 때 간다(./breakpoints.ts).
// - 목록: 열린 문서의 것(지금 줄 — 미저장이면 「저장 전」)과 열지 않은 파일의 Host 목록. 줄을 누르면 그 자리로 가고, 지우기는 그
//   자리에서 지운다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업 규칙).

import type { Breakpoint } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { Trash2 } from "lucide-react";
import { useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { useKeymapPopup } from "../keymap/keymap-provider";
import type { Breakpoints } from "./breakpoints";
import { useDebugStrings } from "./strings";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";
const INPUT =
  "w-full rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5 font-mono text-[12px] text-ui-foreground";

export interface BreakpointUi {
  editing?: { path: string; line: number; x: number; y: number };
  listing: boolean;
}

/** 창의 팝업 상태(debug-feature.tsx 가 그린다). */
export const breakpointUi = createStore<BreakpointUi>(() => ({ listing: false }));

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** 편집 팝업. */
export function BreakpointEditor({
  breakpoints,
  path,
  line,
  x,
  y,
  onClose,
}: {
  breakpoints: Breakpoints;
  path: string;
  line: number;
  x: number;
  y: number;
  onClose(): void;
}) {
  const T = useDebugStrings().breakpoint;
  useKeymapPopup(true);
  const now = breakpoints.lines(path).find((b) => b.line === line);
  const [enabled, setEnabled] = useState(now?.enabled ?? true);
  const [condition, setCondition] = useState(now?.condition ?? "");
  const [logMessage, setLogMessage] = useState(now?.logMessage ?? "");
  const [hitCondition, setHitCondition] = useState(now?.hitCondition ?? "");
  const apply = () => {
    breakpoints.edit(path, line, { enabled, condition, logMessage, hitCondition });
    onClose();
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Content
          data-testid="breakpoint-editor"
          aria-describedby={undefined}
          style={{
            left: Math.min(x, window.innerWidth - 340),
            top: Math.min(y, window.innerHeight - 220),
          }}
          className="fixed z-50 flex w-[320px] flex-col gap-2 rounded-md border border-ui-border bg-ui-panel p-3 text-[12px] text-ui-foreground shadow-lg"
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) {
              e.preventDefault();
              apply();
            }
          }}
        >
          <Dialog.Title className="font-semibold">{T.title(fileName(path), line)}</Dialog.Title>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              data-field="enabled"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
            />
            {T.enabled}
          </label>
          <Field
            label={T.condition}
            name="condition"
            value={condition}
            onChange={setCondition}
            autoFocus
          />
          <Field
            label={T.logMessage}
            name="logMessage"
            value={logMessage}
            onChange={setLogMessage}
          />
          <Field
            label={T.hitCondition}
            name="hitCondition"
            value={hitCondition}
            onChange={setHitCondition}
          />
          <div className="flex justify-between gap-2">
            <button
              type="button"
              className={BUTTON}
              onClick={() => {
                breakpoints.remove(path, line);
                onClose();
              }}
            >
              {T.remove}
            </button>
            <button type="button" className={`${BUTTON} border-ui-accent`} onClick={apply}>
              {T.done}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Field({
  label,
  name,
  value,
  onChange,
  autoFocus,
}: {
  label: string;
  name: string;
  value: string;
  onChange(value: string): void;
  autoFocus?: boolean;
}) {
  return (
    <label className="flex flex-col gap-0.5">
      <span className="text-ui-muted-foreground">{label}</span>
      <input
        data-field={name}
        className={INPUT}
        value={value}
        autoFocus={autoFocus}
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

/** 한 줄의 설명(거터 풍선 도움말 · 목록). */
export function describe(
  T: ReturnType<typeof useDebugStrings>["breakpoint"],
  bp: Breakpoint,
  unverified?: string,
): string {
  const parts: string[] = [];
  if (bp.condition) parts.push(T.describeCondition(bp.condition));
  if (bp.hitCondition) parts.push(T.describeHit(bp.hitCondition));
  if (bp.logMessage) parts.push(T.describeLog(bp.logMessage));
  if (parts.length === 0) parts.push(T.plain);
  if (bp.enabled === false) parts.push(T.disabled);
  if ((bp as { agent?: boolean }).agent === true) parts.push(T.agent);
  if (unverified !== undefined) parts.push(T.unverified(unverified));
  return parts.join("\n");
}

interface Row {
  rel: string;
  /** 열린 문서면 그 절대 경로. */
  path?: string;
  bp: Breakpoint;
  unsaved: boolean;
}

/** 목록(⌘⇧F8). */
export function BreakpointList({
  breakpoints,
  dirty,
  onOpen,
  onClose,
}: {
  breakpoints: Breakpoints;
  dirty(path: string): boolean;
  onOpen(row: { rel: string; path?: string; line: number }): void;
  onClose(): void;
}) {
  const S = useDebugStrings();
  useKeymapPopup(true);
  const state = useStore(breakpoints.store);
  const rows: Row[] = [];
  const openRels = new Set<string>();
  for (const [path, doc] of Object.entries(state.docs)) {
    openRels.add(doc.rel);
    for (const bp of breakpoints.lines(path))
      rows.push({ rel: doc.rel, path, bp, unsaved: dirty(path) });
  }
  for (const [rel, list] of Object.entries(state.saved))
    if (!openRels.has(rel)) for (const bp of list) rows.push({ rel, bp, unsaved: false });
  rows.sort((a, b) => a.rel.localeCompare(b.rel) || a.bp.line - b.bp.line);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="breakpoint-list"
          aria-describedby={undefined}
          className="sheet-top fixed left-1/2 flex max-h-[70vh] w-[560px] max-w-[94vw] -translate-x-1/2 flex-col gap-2 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-[12px] text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{S.list.title}</Dialog.Title>
          {state.problem && <p className="text-ui-error">{S.list.problem(state.problem)}</p>}
          {rows.length === 0 ? (
            <p className="text-ui-muted-foreground">{S.list.empty}</p>
          ) : (
            <ul className="flex min-h-0 flex-col overflow-y-auto">
              {rows.map((row) => (
                <li
                  key={`${row.rel}:${row.bp.line}`}
                  data-breakpoint-row={`${row.rel}:${row.bp.line}`}
                  className="flex items-center gap-2 py-0.5"
                >
                  <input
                    type="checkbox"
                    aria-label={S.breakpoint.enabled}
                    checked={row.bp.enabled !== false}
                    disabled={!row.path}
                    onChange={(e) =>
                      row.path &&
                      breakpoints.edit(row.path, row.bp.line, { enabled: e.target.checked })
                    }
                  />
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-baseline gap-2 text-left hover:underline"
                    onClick={() => onOpen({ rel: row.rel, path: row.path, line: row.bp.line })}
                  >
                    <span className="flex-none font-mono">
                      {row.rel}:{row.bp.line}
                    </span>
                    <span className="truncate text-ui-muted-foreground">
                      {describe(S.breakpoint, row.bp).replace(/\n/g, " · ")}
                    </span>
                    {row.unsaved && (
                      <span className="flex-none text-ui-muted-foreground">{S.list.unsaved}</span>
                    )}
                  </button>
                  <button
                    type="button"
                    aria-label={S.breakpoint.remove}
                    className="flex-none rounded-sm p-0.5 hover:bg-ui-hover"
                    onClick={() =>
                      row.path
                        ? breakpoints.remove(row.path, row.bp.line)
                        : void breakpoints.removeSaved(row.rel, row.bp.line)
                    }
                  >
                    <Trash2 size={12} aria-hidden />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
