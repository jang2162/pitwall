// 전체에서 찾기 팝업(⌘⇧F, 10-ui §2 S5 상단 팝업 · 12-m1-plan M1-20): 입력 · 옵션 토글(대소문자 · 단어 · 정규식) · 범위(프로젝트 /
// 폴더) · 파일 마스크 · 벤더/생성 빼기 → 결과 목록(가상화) → 아래 읽기 전용 미리보기. 바닥 줄에 진행 · 결과 수 · 「더 있음」 · 멈추기 ·
// 「툴 윈도우에서 열기」.
//
// - 칸이나 옵션이 바뀌면 도는 검색을 곧바로 닫고(Host 의 스트림이 끝난다) 잠깐 뒤 새로 찾는다 — 치는 동안 검색이 쌓이지 않게.
// - 키: ↑↓ · PageUp/PageDown 은 결과를 고르고(입력 칸에 포커스가 있어도), Enter 는 고른 결과를 그 줄에서 열고 팝업을 닫는다,
//   ⌘Enter(Win/Linux Ctrl+Enter)는 툴 윈도우로 고정한다, Esc 는 닫는다. 한글 조합 중의 Enter · Esc 는 조합에 쓴다(ui/ime.ts).
// - 팝업이 열린 동안 창 단축키는 꺼진다(`useKeymapPopup`, 액션 찾기와 같다).

import * as Dialog from "@radix-ui/react-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import { CaseSensitive, Regex, Square, WholeWord } from "lucide-react";
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { isWithin, joinPath, separatorOf } from "../file-tree/tree-model";
import { useKeymap, useKeymapPopup } from "../keymap/keymap-provider";
import { useHost } from "../rpc/host";
import { isComposingKey, keepOpenWhileComposing } from "../ui/ime";
import {
  closeFindInFiles,
  openFindInFiles,
  openFindRow,
  pinFindResults,
  searchPlace,
  setFindOptions,
  useFindWindow,
} from "./find-in-files";
import { FindPreview } from "./find-preview";
import {
  canSearch,
  type FindOptions,
  type FindScope,
  type FindSession,
  type FindSnapshot,
  searchParams,
} from "./find-session";
import { baseName, MatchText } from "./match-text";
import { type FindStrings, useFindStrings } from "./strings";

/** 칸을 바꾼 뒤 이만큼 멈춰야 새로 찾는다. 이전 검색은 바꾼 즉시 닫는다. */
export const SEARCH_DELAY_MS = 120;
const ROW_HEIGHT = 24;
const PAGE_ROWS = 10;

export function useFindSnapshot(session: FindSession): FindSnapshot {
  return useSyncExternalStore(session.subscribe, session.getSnapshot);
}

/** 상태 줄: 찾는 중 · 결과 수 · 「더 있음」 · 멈춤 · 실패. */
export function FindStatusText({
  snapshot,
  strings,
}: {
  snapshot: FindSnapshot;
  strings: FindStrings;
}) {
  if (snapshot.status === "idle") return null;
  if (snapshot.status === "failed")
    return (
      <span role="alert" className="truncate text-ui-accent" title={snapshot.error}>
        {snapshot.busy ? strings.busy : strings.failed(snapshot.error ?? "")}
      </span>
    );
  const parts = [strings.summary(snapshot.rows.length, snapshot.files.length)];
  if (snapshot.status === "searching") parts.unshift(strings.searching);
  if (snapshot.status === "cancelled") parts.push(strings.cancelled);
  return (
    <span data-testid="find-status" className="truncate">
      {parts.join(" · ")}
      {snapshot.truncated && (
        <span data-testid="find-more" className="ml-2 text-ui-accent">
          {strings.more}
        </span>
      )}
    </span>
  );
}

function Toggle({
  pressed,
  label,
  onChange,
  children,
}: {
  pressed: boolean;
  label: string;
  onChange(pressed: boolean): void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={pressed}
      onClick={() => onChange(!pressed)}
      className={`flex h-6 min-w-6 items-center justify-center rounded-sm px-1 text-[12px] ${
        pressed ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground hover:bg-ui-hover"
      }`}
    >
      {children}
    </button>
  );
}

const INPUT_CLASS =
  "min-w-0 rounded-sm border border-ui-border bg-transparent px-2 py-0.5 text-[12px] outline-none placeholder:text-ui-muted-foreground focus:border-ui-accent";

/** 폴더 칸의 글자 ↔ 절대 경로. 칸에는 루트 기준 상대 경로를 쓴다(루트 밖이면 절대 경로 그대로). */
function folderText(path: string, root: string | undefined): string {
  if (root === undefined || !isWithin(path, root)) return path;
  const sep = separatorOf(root);
  return path === root ? "" : path.slice(root.endsWith(sep) ? root.length : root.length + 1);
}

function folderPath(text: string, root: string | undefined): string {
  const t = text.trim();
  if (root === undefined || /^([/\\]|[A-Za-z]:[/\\])/.test(t)) return t;
  return t === "" ? root : joinPath(root, t);
}

function folderScope(text: string): FindScope {
  const root = searchPlace()?.root;
  return { kind: "folder", path: folderPath(text, root), root };
}

export function FindPopup({ workspace }: { workspace: EditorWorkspace }) {
  const open = useFindWindow((s) => s.open);
  useKeymapPopup(open);
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => (next ? openFindInFiles() : closeFindInFiles())}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        {open && <FindPopupContent workspace={workspace} />}
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function FindPopupContent({ workspace }: { workspace: EditorWorkspace }) {
  const T = useFindStrings();
  const { platform } = useKeymap();
  const { client } = useHost();
  const options = useFindWindow((s) => s.options);
  const session = useFindWindow((s) => s.session);
  const snapshot = useFindSnapshot(session);
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const root = searchPlace()?.root;
  const [folder, setFolder] = useState(() =>
    options.scope.kind === "folder" ? folderText(options.scope.path, root) : "",
  );

  const update = (next: Partial<FindOptions>) => setFindOptions({ ...options, ...next });

  // 옵션이 바뀌면 이전 검색을 곧바로 닫고, 멈춘 뒤에 찾는다. 루트를 아직 모르면 기다린다.
  useEffect(() => {
    if (!canSearch(options)) {
      session.clear();
      return;
    }
    session.interrupt();
    const timer = setTimeout(() => {
      const place = searchPlace();
      if (place) void session.start(searchParams(options, place));
      else session.cancel();
    }, SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [options, session]);

  // 새 검색이 시작되면 첫 줄을 고른다.
  const params = snapshot.params;
  useEffect(() => setSelected(0), [params]);

  const rows = snapshot.rows;
  const row = rows[Math.min(selected, rows.length - 1)];
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  // 키로 고른 줄만 화면 안으로 굴린다 — 결과가 들어올 때마다 사람이 굴린 자리를 빼앗지 않게.
  const [scrollRequest, setScrollRequest] = useState(0);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  useLayoutEffect(() => {
    if (scrollRequest > 0) virtualizer.scrollToIndex(selectedRef.current);
  }, [scrollRequest, virtualizer]);

  const openRow = (index: number) => {
    const target = rows[index];
    if (!target) return;
    closeFindInFiles();
    openFindRow(workspace, target, params?.worktree ?? undefined);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (isComposingKey(e.nativeEvent)) return;
    const last = rows.length - 1;
    const move = (to: number) => {
      setSelected(Math.max(0, Math.min(last, to)));
      setScrollRequest((n) => n + 1);
    };
    switch (e.key) {
      case "ArrowDown":
        move(selected + 1);
        break;
      case "ArrowUp":
        move(selected - 1);
        break;
      case "PageDown":
        move(selected + PAGE_ROWS);
        break;
      case "PageUp":
        move(selected - PAGE_ROWS);
        break;
      case "Enter":
        if (platform === "mac" ? e.metaKey : e.ctrlKey) pinFindResults(workspace.api());
        else if (e.target instanceof HTMLButtonElement) return;
        else openRow(selected);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const pinKey = platform === "mac" ? "⌘↩" : "Ctrl+Enter";

  return (
    <Dialog.Content
      aria-label={T.action}
      aria-describedby={undefined}
      data-find-popup=""
      onEscapeKeyDown={keepOpenWhileComposing}
      onKeyDown={onKeyDown}
      className="fixed top-[10%] left-1/2 flex w-[760px] max-w-[94vw] -translate-x-1/2 flex-col overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
    >
      <Dialog.Title className="sr-only">{T.action}</Dialog.Title>
      <div className="flex items-center gap-1 border-b border-ui-border px-2 py-1">
        <input
          autoFocus
          aria-label={T.placeholder}
          placeholder={T.placeholder}
          value={options.query}
          onChange={(e) => update({ query: e.target.value })}
          className="min-w-0 flex-1 bg-transparent px-1 py-1 text-[13px] outline-none placeholder:text-ui-muted-foreground"
        />
        <Toggle
          pressed={options.matchCase}
          label={T.matchCase}
          onChange={(matchCase) => update({ matchCase })}
        >
          <CaseSensitive size={15} aria-hidden />
        </Toggle>
        <Toggle
          pressed={options.wholeWord}
          label={T.wholeWord}
          onChange={(wholeWord) => update({ wholeWord })}
        >
          <WholeWord size={15} aria-hidden />
        </Toggle>
        <Toggle pressed={options.regex} label={T.regex} onChange={(regex) => update({ regex })}>
          <Regex size={15} aria-hidden />
        </Toggle>
      </div>
      <div className="flex items-center gap-2 border-b border-ui-border px-2 py-1 text-[12px]">
        <div role="radiogroup" className="flex flex-none gap-0.5">
          {(["project", "folder"] as const).map((kind) => (
            <button
              key={kind}
              type="button"
              role="radio"
              aria-checked={options.scope.kind === kind}
              onClick={() =>
                update({
                  scope: kind === "project" ? { kind } : folderScope(folder),
                })
              }
              className={`rounded-sm px-2 py-0.5 ${
                options.scope.kind === kind
                  ? "bg-ui-active"
                  : "text-ui-muted-foreground hover:bg-ui-hover"
              }`}
            >
              {kind === "project" ? T.scopeProject : T.scopeFolder}
            </button>
          ))}
        </div>
        {options.scope.kind === "folder" && (
          <input
            aria-label={T.folder}
            title={T.folder}
            value={folder}
            onChange={(e) => {
              setFolder(e.target.value);
              update({
                scope: folderScope(e.target.value),
              });
            }}
            className={`${INPUT_CLASS} flex-1`}
          />
        )}
        <input
          aria-label={T.mask}
          title={T.mask}
          placeholder={T.maskPlaceholder}
          value={options.mask}
          onChange={(e) => update({ mask: e.target.value })}
          className={`${INPUT_CLASS} ${options.scope.kind === "folder" ? "w-[160px]" : "flex-1"}`}
        />
        <Toggle
          pressed={options.excludeVendored}
          label={T.excludeVendored}
          onChange={(excludeVendored) => update({ excludeVendored })}
        >
          <span aria-hidden>{T.excludeVendored}</span>
        </Toggle>
      </div>
      <div
        ref={listRef}
        role="listbox"
        aria-label={T.action}
        aria-activedescendant={row ? `find-row-${selected}` : undefined}
        className="h-[240px] flex-none overflow-y-auto"
      >
        {rows.length === 0 ? (
          snapshot.status === "done" && (
            <p className="px-3 py-2 text-[12px] text-ui-muted-foreground">{T.empty}</p>
          )
        ) : (
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((item) => {
              const r = rows[item.index];
              if (!r) return null;
              const isSelected = item.index === selected;
              return (
                <div
                  key={item.key}
                  id={`find-row-${item.index}`}
                  role="option"
                  aria-selected={isSelected}
                  data-find-row={`${r.file.path}:${r.line.line}`}
                  onClick={() => setSelected(item.index)}
                  onDoubleClick={() => openRow(item.index)}
                  style={{ transform: `translateY(${item.start}px)`, height: ROW_HEIGHT }}
                  className={`absolute top-0 left-0 flex w-full cursor-default items-center gap-3 px-3 ${
                    isSelected ? "bg-ui-active" : "hover:bg-ui-hover"
                  }`}
                >
                  <MatchText line={r.line} />
                  <span
                    title={r.file.path}
                    className="max-w-[40%] flex-none truncate text-[11px] text-ui-muted-foreground"
                  >
                    {baseName(r.file.path)} {r.line.line}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
      <FindPreview
        client={client}
        projectId={params?.projectId}
        worktree={params?.worktree}
        row={row}
        strings={T}
      />
      <div className="flex h-[28px] flex-none items-center gap-2 border-t border-ui-border px-3 text-[12px] text-ui-muted-foreground">
        <span className="flex min-w-0 flex-1">
          <FindStatusText snapshot={snapshot} strings={T} />
        </span>
        {snapshot.status === "searching" && (
          <button
            type="button"
            onClick={() => session.cancel()}
            className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover hover:text-ui-foreground"
          >
            <Square size={10} aria-hidden />
            {T.stop}
          </button>
        )}
        <button
          type="button"
          disabled={snapshot.status === "idle"}
          onClick={() => pinFindResults(workspace.api())}
          className="rounded-sm px-1 hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-40"
        >
          {T.pin} <kbd className="font-sans">{pinKey}</kbd>
        </button>
      </div>
    </Dialog.Content>
  );
}
