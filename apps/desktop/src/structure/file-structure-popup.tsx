// 파일 구조 팝업(⌘F12 · Win/Linux Ctrl+F12, 15-m4-plan M4-10 · 10-ui §4.2): 구조 툴 윈도우와 같은 트리(창의 아웃라인 —
// outline-service.ts)를 상단 중앙 팝업(cmdk — 10-ui §3)에 보이고 이름을 퍼지로 거른다(`filterRows` — 맞은 심볼과 흐린 조상).
// `↑` `↓` · `↩`(그 이름 자리로 위치로 가기 — 기록에 넣는다), `Esc` · 바깥 누르기는 닫고 포커스를 에디터로 돌린다.
//
// - 열면 커서를 담은 가장 안쪽 심볼이 골라져 있다. 거르면 가장 잘 맞는 줄(같음 · 앞부분 · 이어진 부분 · 차례로 듦 순)을 고른다.
// - 줄이 많으면(1만 심볼) `POPUP_ROWS` 줄만 그리고 「n줄 더」 를 적는다 — cmdk 는 그린 줄로 키 이동을 하므로 가상화 대신 거르기로
//   좁힌다. 어느 줄을 그릴지는 `filterRows` 가 정한다: 거르면 잘 맞는 것부터, 질의가 비면 커서의 심볼 둘레. 툴 윈도우는 가상화로
//   모두 보인다.
// - `Esc` 는 Radix Dialog 가 처리하며 `preventDefault` 한다 — 같은 눌림이 창의 `Esc`(멀티 커서 접기 — M4-14)로 가지 않는다. 한글
//   조합을 취소하는 `Esc` 는 팝업을 닫지 않는다(ui/ime.ts). 떠 있는 동안 창 단축키는 쉰다(`useKeymapPopup`).
// - 파일 창(언어 서버 · 인덱스를 부를 프로젝트가 없다)에서는 액션이 비활성이다 — 액션 찾기에 흐리게, 키를 가져가지 않는다
//   (메인 판정 2026-11-02).
// - 출처(LSP · 서버 / 인덱스)를 제목 줄에 흐리게 적는다.

import type { Action } from "@pitwall/keymap";
import * as Dialog from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import { type ReactNode, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import type { EditorWorkspace } from "../editor/editor-workspace";
import { keepOpenWhileComposing } from "../ui/ime";
import { filterRows, innermostAt } from "./outline";
import { outlineService } from "./outline-service";
import { useOutline } from "./structure-tool-window";
import { goToSymbol, SourceLabel, SymbolLabel } from "./symbol-row";
import { emptyText, type StructureStrings, useStructureStrings } from "./strings";

/** cmdk 가 고른 줄을 옮기는 키(`ctrl+n/j/p/k` 와 함께). */
const MOVE_KEYS = new Set(["ArrowDown", "ArrowUp", "Home", "End"]);

/** 팝업이 한 번에 그리는 줄 수. */
export const POPUP_ROWS = 300;

export const FILE_STRUCTURE_ACTION = "navigate.fileStructure";

export function fileStructureActions(deps: {
  workspace: EditorWorkspace;
  strings: StructureStrings;
  open(): void;
}): Action[] {
  return [
    {
      id: FILE_STRUCTURE_ACTION,
      label: deps.strings.fileStructure,
      keywords: ["File Structure", "Outline"],
      // 파일 창에는 프로젝트가 없어 아웃라인을 읽을 길이 없다.
      enabled: () => deps.workspace.lsp !== undefined,
      run: deps.open,
    },
  ];
}

/** 창에 파일 구조 팝업을 붙인다. 돌려준 요소를 창이 그린다. */
export function useFileStructurePopup(workspace: EditorWorkspace): ReactNode {
  const T = useStructureStrings();
  const [open, setOpen] = useState(false);
  const openPopup = useCallback(() => {
    if (workspace.activeTab()) setOpen(true);
  }, [workspace]);
  const actions = useMemo(
    () => fileStructureActions({ workspace, strings: T, open: openPopup }),
    [workspace, T, openPopup],
  );
  useRegisterActions(actions);
  useKeymapPopup(open);
  return open ? (
    <FileStructurePopup workspace={workspace} strings={T} onClose={() => setOpen(false)} />
  ) : null;
}

function FileStructurePopup({
  workspace,
  strings: T,
  onClose,
}: {
  workspace: EditorWorkspace;
  strings: StructureStrings;
  onClose(): void;
}) {
  const service = outlineService(workspace);
  const state = useOutline(service, true, workspace.api());
  const outline = state.status === "ready" ? state.outline : undefined;
  const [query, setQuery] = useState("");
  // 사람이 고른 줄(↑↓ · 마우스). 없으면 — 처음은 커서의 심볼, 거르면 가장 잘 맞는 줄(`best` — 같음 · 앞부분 · 이어진 부분 ·
  // 차례로 듦 순). 이 값을 렌더에서 바로 정한다: 항목이 처음 그려지는 커밋에 이미 들어 있어야 cmdk 가 자기 규칙(첫 줄)으로
  // 고르지 않는다. 커서는 처음 한 번만 본다(떠 있는 동안 에디터는 키를 받지 않는다).
  const [picked, setPicked] = useState<string>();
  const cursor = useRef(state.cursor);
  cursor.current ??= state.cursor;
  const atCursor = useMemo(
    () =>
      outline && cursor.current !== undefined ? innermostAt(outline, cursor.current) : undefined,
    [outline],
  );
  // 커서의 심볼은 그린 줄 안에 든다 — 앞 `POPUP_ROWS` 줄 밖이면 그 둘레를 그린다(M4-10 재리뷰 N2).
  const {
    rows: shown,
    total,
    best,
  } = useMemo(
    () =>
      outline
        ? filterRows(outline, query, "position", POPUP_ROWS, atCursor)
        : { rows: [], total: 0, best: undefined },
    [outline, query, atCursor],
  );
  // cmdk 는 질의가 바뀌거나 고른 줄이 사라지면 스스로 첫 줄을 고른다 — 자기 값을 먼저 바꾸고 `onValueChange` 로 알린다. 그것이
  // 가장 잘 맞는 줄을 덮지 않게 사람이 고른 것(↑↓ · Home · End · 마우스)만 `picked` 로 받는다. cmdk 가 스스로 고른 값(`auto`)은 한
  // 번 그대로 넘겼다가 그리기 전(layout effect)에 거둔다 — 넘긴 `value` 가 바뀌어야 cmdk 가 그 값을 다시 읽는다.
  const byUser = useRef(false);
  const [auto, setAuto] = useState<string>();
  const pick = (next: string) => {
    if (byUser.current) {
      byUser.current = false;
      setPicked(next);
    } else setAuto(next);
  };
  useLayoutEffect(() => {
    if (auto !== undefined) setAuto(undefined);
  }, [auto]);
  // 가장 잘 맞는 줄은 등급 순으로 가장 먼저 채워지므로 늘 그린 줄 안에 있다.
  const value =
    auto ??
    picked ??
    (query === "" ? atCursor : best)?.toString() ??
    shown.find((r) => r.match)?.id.toString() ??
    "";
  const filter = (next: string) => {
    byUser.current = false;
    setQuery(next);
    setPicked(undefined);
  };

  const go = (id: number) => {
    const symbol = outline?.symbols[id];
    if (!symbol) return;
    onClose();
    void goToSymbol(workspace, state, symbol);
  };

  return (
    <Dialog.Root open onOpenChange={(next) => !next && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay cmdk-overlay="" className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          cmdk-dialog=""
          aria-label={T.fileStructure}
          aria-describedby={undefined}
          onEscapeKeyDown={keepOpenWhileComposing}
          data-file-structure=""
          className="fixed top-[15%] left-1/2 w-[560px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-[12px] text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="sr-only">{T.fileStructure}</Dialog.Title>
          <Command
            label={T.fileStructure}
            shouldFilter={false}
            value={value}
            onValueChange={pick}
            onKeyDown={(e) => {
              byUser.current = MOVE_KEYS.has(e.key) || (e.ctrlKey && "njpk".includes(e.key));
            }}
            loop
          >
            <div className="flex items-center gap-2 border-b border-ui-border px-3 py-1">
              <span className="font-medium">{T.fileStructure}</span>
              <span className="min-w-0 flex-1 truncate text-right">
                {outline && <SourceLabel outline={outline} strings={T} />}
              </span>
            </div>
            <Command.Input
              autoFocus
              value={query}
              onValueChange={filter}
              placeholder={T.popupPlaceholder}
              className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
            />
            <Command.List
              onPointerMoveCapture={() => (byUser.current = true)}
              className="max-h-[360px] overflow-y-auto py-1"
            >
              {outline ? (
                <Command.Empty className="px-3 py-2 text-ui-muted-foreground">
                  {T.popupEmpty}
                </Command.Empty>
              ) : (
                <p className="px-3 py-2 text-ui-muted-foreground" data-structure-empty="">
                  {emptyText(T, state.status)}
                </p>
              )}
              {outline &&
                shown.map((row) => {
                  const symbol = outline.symbols[row.id]!;
                  return (
                    <Command.Item
                      key={row.id}
                      value={String(row.id)}
                      disabled={!row.match}
                      onSelect={() => go(row.id)}
                      aria-label={T.row(symbol.name, T.kinds[symbol.kind])}
                      data-structure-item={row.id}
                      style={{ paddingLeft: 12 + row.depth * 12 }}
                      className="flex h-[24px] cursor-default items-center gap-1 pr-3 data-[selected=true]:bg-ui-active"
                    >
                      <SymbolLabel symbol={symbol} strings={T} dim={!row.match} />
                    </Command.Item>
                  );
                })}
              {outline && state.serverFailed && (
                <p
                  className="px-3 py-1 text-[11px] text-ui-muted-foreground"
                  data-structure-notice=""
                >
                  {T.serverFailed}
                </p>
              )}
              {total > shown.length && (
                <p
                  className="px-3 py-1 text-[11px] text-ui-muted-foreground"
                  data-structure-more=""
                >
                  {T.popupMore(total - shown.length)}
                </p>
              )}
            </Command.List>
          </Command>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
