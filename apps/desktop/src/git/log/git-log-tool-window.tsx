// Git 로그 툴 윈도우(⌘9 — docs/design/13-m2-plan.md M2-14, 10-ui.md 「주요 툴 윈도우 상세」 의 Git 로그 그림): 왼쪽 브랜치 패널 ·
// 가운데 필터 바 + 그래프 목록(canvas) · 오른쪽 커밋 상세. 하단 dock 이 기본 자리이고, 탭을 가운데로 끌면 에디터 영역에서 크게
// 본다(툴 윈도우 패널 그대로 — project-layout.ts 의 `centre` 자리).
//
// - 로그는 이 창의 활성 워크트리 · 필터로 `git.log` 를 받고(log-model.ts), 터미널 · 에이전트가 ref 를 바꾸면(`git.refsChanged`, 이
//   창의 `git.watch` 가 낸다) 같은 필터로 다시 받는다 — 고른 커밋은 해시로 남는다.
// - 브랜치 패널 · 필터 바 · 라벨 메뉴의 ref 는 따로 읽지 않고 창의 브랜치 상태(branch-state.ts — 상태바 · 브랜치 팝업과 같은
//   `git.refs` 읽기 하나)를 쓴다.
// - 쓰기(우클릭 · 라벨 끌어 놓기)는 log-ops.ts. ref 를 옮긴 결과가 오면 목록 위에 [되돌리기] 줄을 단다.
// - 다른 화면의 「로그에서 선택」(`git.log.reveal`, log-reveal.ts)을 받아 그 커밋을 고른다 — 지금 필터에 없으면 필터를 푼다.
// - 커밋 상세 탭 · 두 커밋 비교 탭 · 리비전 파일 열기는 M2-15 의 액션을 id 로 부른다(`registry.run(id, args)`). 셋 다 인자가
//   있어야 도는 액션이라 `isEnabled` 는 늘 거짓이다 — 메뉴 항목 · 버튼을 둘지는 `isAvailable` 로 정한다(없는 창이면 감춘다).

import * as ContextMenu from "@radix-ui/react-context-menu";
import type { GitRefMove, GitRefs, LogRef, LogRow, ResetMode } from "@pitwall/rpc-client";
import type { IDockviewPanelProps } from "dockview-react";
import { type ReactNode, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { useFileDialogs } from "../../file-tree/file-dialogs";
import { useKeymap } from "../../keymap/keymap-provider";
import {
  type NotificationHandle,
  useNotify,
  useRetirableNotify,
} from "../../notifications/notifications";
import { useActiveWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { useTheme } from "../../theme/theme-switcher";
import { useBranch, windowBranch } from "../branch-state";
import { BranchPanel } from "./branch-panel";
import { CommitDetail } from "./commit-detail";
import { FilterBar } from "./filter-bar";
import { paletteOf } from "./graph-render";
import { type DropTarget, LogList } from "./log-list";
import { EMPTY_QUERY, LogModel, type LogQuery, type LogSource, logParams } from "./log-model";
import { useGitDialogs } from "../dialogs/git-dialogs";
import { useGitDialogStrings } from "../dialogs/strings";
import { type UndoKind, canUndo } from "../dialogs/undo";
import { LogOps, refFullName, short } from "./log-ops";
import { logRevealRequests } from "./log-reveal";
import { useGitLogStrings } from "./strings";

const RESET_MODES: readonly ResetMode[] = ["soft", "mixed", "keep", "hard"];

/** 우클릭 메뉴의 대상: 행(과 그 위 라벨) 또는 끌어 놓은 라벨. */
type MenuTarget =
  | { kind: "row"; row: LogRow; ref?: LogRef }
  | { kind: "drop"; source: LogRef; from: LogRow; target: DropTarget };

function MenuItem({
  onSelect,
  disabled,
  children,
}: {
  onSelect(): void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <ContextMenu.Item
      onSelect={onSelect}
      disabled={disabled}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
    >
      {children}
    </ContextMenu.Item>
  );
}

const SEPARATOR = <ContextMenu.Separator className="my-1 h-px bg-ui-border" />;

/** 글꼴: 앱 본문의 글꼴로 12 px. */
function listFont(): string {
  const family = typeof document !== "undefined" ? getComputedStyle(document.body).fontFamily : "";
  return `12px ${family || "system-ui, sans-serif"}`;
}

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });
const formatDate = (seconds: number) => dateFormat.format(new Date(seconds * 1000));

/** 고르기: 클릭 · ⌘ · ⇧ 를 고른 해시 목록(로그 차례)으로. */
export function selectRows(
  rows: readonly LogRow[],
  selected: readonly string[],
  anchor: number,
  index: number,
  mode: "single" | "toggle" | "range",
): { selected: string[]; anchor: number } {
  const hash = rows[index]!.hash;
  if (mode === "toggle") {
    const next = selected.includes(hash) ? selected.filter((h) => h !== hash) : [...selected, hash];
    return { selected: next, anchor: index };
  }
  if (mode === "range" && anchor >= 0 && anchor < rows.length) {
    const [from, to] = anchor <= index ? [anchor, index] : [index, anchor];
    return { selected: rows.slice(from, to + 1).map((r) => r.hash), anchor };
  }
  return { selected: [hash], anchor: index };
}

/** 필터가 하나도 없는가(브랜치 · 글자 칸이 모두 비었다). */
function isEmptyQuery(query: LogQuery): boolean {
  return (
    query.refs.length === 0 &&
    [query.text, query.author, query.since, query.until, query.paths].every((v) => v.trim() === "")
  );
}

/** 커밋이 모든 ref(브랜치 · 원격 · 태그 · HEAD)에서 닿는가 — 거르지 않은 로그에 들까. 묻지 못하면 거짓. */
async function reachable(source: LogSource, hash: string): Promise<boolean> {
  const { filter } = logParams({ ...EMPTY_QUERY, text: hash }, true);
  try {
    const page = await source.client.call("git.logPage", {
      projectId: source.projectId,
      worktree: source.worktree ?? null,
      refs: null,
      filter,
      order: "topo",
      limit: 1,
    });
    return page.rows.length > 0;
  } catch (e) {
    console.warn("[git] could not check whether the commit is in the log", e);
    return false;
  }
}

export interface GitLogViewProps {
  /** 성능 측정: 그린 뒤 부른다. */
  onDrawn?(): void;
}

export function GitLogToolWindow({ api }: IDockviewPanelProps) {
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      <GitLogView />
    </div>
  );
}

export function GitLogView({ onDrawn }: GitLogViewProps) {
  const T = useGitLogStrings();
  const { client, target } = useHost();
  const projectId = "projectId" in target ? target.projectId : undefined;
  const worktree = useActiveWorktree();
  const { registry } = useKeymap();
  const notify = useNotify();
  const dialogs = useFileDialogs();
  const gitDialogs = useGitDialogs();
  const D = useGitDialogStrings();
  const theme = useTheme((s) => s.theme);
  const appearance = useTheme((s) => s.appearance);
  const palette = useMemo(() => {
    const values = theme.variants[appearance];
    return values ? paletteOf(values) : undefined;
  }, [theme, appearance]);
  const font = useMemo(listFont, []);
  const [model] = useState(() => new LogModel());
  const [query, setQuery] = useState<LogQuery>(EMPTY_QUERY);
  const { refs } = useBranch();
  const [selection, setSelection] = useState<{ selected: string[]; anchor: number }>({
    selected: [],
    anchor: -1,
  });
  const [moves, setMoves] = useState<{ move: GitRefMove; kind: UndoKind }[]>([]);
  const [menu, setMenu] = useState<MenuTarget>();
  const [reveal, setReveal] = useState<{ index: number; seq: number }>();
  const [, setTick] = useState(0);
  const triggerRef = useRef<HTMLDivElement>(null);
  const pendingDrop = useRef<MenuTarget | undefined>(undefined);

  const source: LogSource | undefined = useMemo(
    () => (client && projectId ? { client, projectId, worktree } : undefined),
    [client, projectId, worktree],
  );

  useEffect(() => () => model.dispose(), [model]);
  useEffect(() => model.subscribe(() => setTick((t) => t + 1)), [model]);

  useEffect(() => {
    if (!source) return;
    void model.load(source, query);
  }, [model, source, query]);

  // ref 가 바뀌면 같은 필터로 다시 받는다.
  const queryRef = useRef(query);
  queryRef.current = query;
  useEffect(() => {
    if (!source) return;
    let stopped = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    void source.client
      .subscribe({ projectId: source.projectId, topics: ["git.refsChanged"] }, (event) => {
        const changed = event.payload as { worktree?: string | null } | null;
        if (stopped || (changed?.worktree ?? undefined) !== source.worktree) return;
        void model.load(source, queryRef.current);
      })
      .then(
        (s) => {
          unsubscribe = () => s.unsubscribe();
          if (stopped) void unsubscribe().catch(() => {});
        },
        (e: unknown) => console.warn("[git] could not follow ref changes", e),
      );
    return () => {
      stopped = true;
      void unsubscribe?.().catch(() => {});
    };
  }, [model, source]);

  const ops = useMemo(
    () =>
      source
        ? new LogOps({
            client: source.client,
            projectId: source.projectId,
            worktree: source.worktree,
            askName: dialogs.askName,
            askBranch: gitDialogs.askBranch,
            confirm: gitDialogs.confirm,
            notify,
            strings: T,
            dialogStrings: D,
            branches: () => new Set((windowBranch().get().refs?.branches ?? []).map((b) => b.name)),
            moved: (move, kind) =>
              setMoves((m) => [
                { move, kind },
                ...m.filter((x) => x.move.refName !== move.refName),
              ]),
          })
        : undefined,
    [source, dialogs.askName, gitDialogs.askBranch, gitDialogs.confirm, notify, T, D],
  );

  // 고른 것은 로그 차례로(상세 · cherry-pick 이 차례를 쓴다). 지금 목록에 없는 해시는 빠진다.
  // 고른 수는 적다 — 렌더마다 센다(행이 더 오면 그때 나타난 해시도 든다).
  const selected = selection.selected
    .map((h) => ({ h, i: model.indexOf(h) }))
    .filter((x) => x.i >= 0)
    .sort((a, b) => a.i - b.i)
    .map((x) => x.h);
  const selectedKey = selected.join(",");
  // 목록은 이 집합이 바뀔 때만 다시 그린다.
  const selectedSet = useMemo(
    () => new Set(selectedKey === "" ? [] : selectedKey.split(",")),
    [selectedKey],
  );

  // 액션이 등록 · 해제되면 다시 그린다(메뉴 항목을 둘지가 바뀐다).
  useSyncExternalStore(registry.subscribe, registry.getVersion);
  const openCommit = registry.isAvailable("git.showCommit")
    ? (rev: string) => void registry.run("git.showCommit", { rev })
    : undefined;
  const compare = registry.isAvailable("git.compareCommits")
    ? (a: string, b: string) => void registry.run("git.compareCommits", { a, b })
    : undefined;
  const openRevisionFile = registry.isAvailable("git.openRevisionFile")
    ? (rev: string, path: string) => void registry.run("git.openRevisionFile", { rev, path })
    : undefined;
  const copyHashes = (hashes: readonly string[]) => {
    if (hashes.length === 0) return;
    navigator.clipboard.writeText(hashes.join("\n")).then(
      () => notify({ level: "info", title: T.result.copied }),
      (e: unknown) => console.warn("[git] could not copy", e),
    );
  };
  const selectHash = (hash: string) => {
    const index = model.indexOf(hash);
    if (index < 0) return;
    setSelection({ selected: [hash], anchor: index });
    setReveal({ index, seq: (reveal?.seq ?? 0) + 1 });
  };

  // 「로그에서 선택」(log-reveal.ts — 다른 화면이 `git.log.reveal { rev }` 로 부른다): 요청을 받아 커밋 해시로 풀고, 그 행이 목록에
  // 오면 고르고 목록에 포커스를 준다. 받은 목록에 없고 `git.log` 가 끝났으면 모든 ref 에서 닿는지 묻고, 닿으면 필터를 풀어 다시
  // 받는다(그 뒤에도 없으면 「없다」). 이 효과들은 로그를 받는 효과(위) 뒤에 둔다 — 필터를 풀면 같은 커밋에서 받기가 먼저 시작돼야
  // 끝난 옛 목록을 보고 「없다」 고 하지 않는다.
  const [pending, setPending] = useState<{ rev: string; hash?: string; cleared: boolean }>();
  useEffect(() => {
    const take = () => {
      const { rev } = logRevealRequests.getState();
      if (rev === undefined) return;
      logRevealRequests.setState({ rev: undefined });
      setPending({ rev, cleared: false });
    };
    take();
    return logRevealRequests.subscribe(take);
  }, []);
  const notifyRetirable = useRetirableNotify();
  const latest = useRef({ notify, notifyRetirable, T });
  latest.current = { notify, notifyRetirable, T };
  // 「로그에서 선택」 이 푼 필터와 그 알림. 필터가 그 뒤 바뀌면(되돌리기 · 사람의 고침) 알림의 단추를 끈다 — 창이 닫혀도.
  const clearedFilter = useRef<{ cleared: LogQuery; handle: NotificationHandle }>(undefined);
  useEffect(() => {
    const c = clearedFilter.current;
    if (c && c.cleared !== query) {
      c.handle.retire();
      clearedFilter.current = undefined;
    }
  }, [query]);
  useEffect(() => () => clearedFilter.current?.handle.retire(), []);
  useEffect(() => {
    if (!pending || pending.hash !== undefined || !source) return;
    let current = true;
    source.client
      .call("git.show", {
        projectId: source.projectId,
        worktree: source.worktree ?? null,
        rev: pending.rev,
      })
      .then(
        (show) => current && setPending((p) => (p === pending ? { ...p, hash: show.commit } : p)),
        (e: unknown) => {
          if (!current) return;
          setPending(undefined);
          const { notify, T } = latest.current;
          notify({
            level: "error",
            title: T.reveal.failed(e instanceof Error ? e.message : String(e)),
          });
        },
      );
    return () => {
      current = false;
    };
  }, [pending, source]);
  const rootRef = useRef<HTMLDivElement>(null);
  const version = model.getVersion();
  useEffect(() => {
    const hash = pending?.hash;
    if (!pending || hash === undefined || !source) return;
    const { notify, T } = latest.current;
    const missing = () => {
      setPending(undefined);
      notify({
        level: "warning",
        title: T.reveal.missing,
        body: T.reveal.missingBody(short(hash)),
      });
    };
    if (model.indexOf(hash) >= 0) {
      setPending(undefined);
      selectHash(hash);
      requestAnimationFrame(() =>
        rootRef.current?.querySelector<HTMLElement>('[role="grid"]')?.focus(),
      );
      return;
    }
    if (model.status.kind !== "done" && model.status.kind !== "error") return;
    if (pending.cleared || isEmptyQuery(query)) return missing();
    let current = true;
    void reachable(source, hash).then((yes) => {
      if (!current) return;
      if (!yes) return missing();
      setPending({ ...pending, cleared: true });
      // 푼 필터는 알림의 [필터 되돌리기] 로 한 번 되살린다. 그사이 사람이 필터를 바꾸면 단추를 끈다(아래 효과).
      const previous = query;
      const cleared = { ...EMPTY_QUERY };
      setQuery(cleared);
      const handle = latest.current.notifyRetirable({
        level: "info",
        title: T.reveal.cleared,
        body: T.reveal.clearedBody,
        action: {
          label: T.reveal.restore,
          run: () => {
            if (queryRef.current === cleared) setQuery(previous);
          },
        },
      });
      clearedFilter.current = { cleared, handle };
    });
    return () => {
      current = false;
    };
    // `selectHash` 는 렌더마다 새로 만든다 — 행이 오거나(`version`) 요청 · 필터가 바뀔 때만 본다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, version, query, source, model]);

  const onDropLabel = (
    source_: LogRef,
    from: LogRow,
    target: DropTarget,
    at: { x: number; y: number },
  ) => {
    // 끌어 놓기 메뉴는 우클릭 메뉴와 같은 메뉴를 놓은 자리에 띄운다.
    pendingDrop.current = { kind: "drop", source: source_, from, target };
    triggerRef.current?.dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: at.x,
        clientY: at.y,
      }),
    );
  };

  const status =
    model.status.kind === "error"
      ? T.status.failed(model.status.message)
      : model.status.kind === "loading" && model.rows.length === 0
        ? T.status.loading
        : model.status.kind === "done" && model.rows.length === 0
          ? T.status.empty
          : model.filtered
            ? (model.textAsHash ? T.status.matchedHash : T.status.matched)(model.rows.length)
            : T.status.count(model.total ?? model.rows.length);

  if (!projectId)
    return <p className="p-2 text-[12px] text-ui-muted-foreground">{T.status.noProject}</p>;

  return (
    <div ref={rootRef} className="flex h-full min-h-0">
      <div className="w-[clamp(120px,18%,220px)] shrink-0 border-r border-ui-border">
        <BranchPanel
          refs={refs}
          chosen={query.refs}
          onChoose={(chosen) => setQuery({ ...query, refs: chosen })}
          onCheckout={(section, name) =>
            section === "local"
              ? void ops?.checkoutBranch(name, T.menu.checkoutBranch(name))
              : void ops?.checkoutRemote(name, T.menu.checkoutBranch(name))
          }
          strings={T.branches}
        />
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <FilterBar query={query} onChange={setQuery} refs={refs} strings={T.filter} />
        {moves.map(({ move, kind }) => (
          <div
            key={move.refName}
            role="status"
            className="flex items-center gap-2 border-b border-ui-border px-2 py-0.5 text-[12px]"
          >
            <span className="truncate">
              {T.result.moved(
                move.refName.replace(/^refs\/heads\//, ""),
                move.previous ? short(move.previous) : "∅",
                move.current ? short(move.current) : "∅",
              )}
            </span>
            {canUndo(move, kind) && (
              <button
                type="button"
                className="rounded-sm border border-ui-border px-2 hover:bg-ui-hover"
                onClick={() => {
                  // 알림(되돌렸다 · 실패)은 undo 가 낸다.
                  void ops?.undo(move, kind).then((done) => {
                    if (done) setMoves((m) => m.filter((x) => x.move !== move));
                  });
                }}
              >
                {T.result.undo}
              </button>
            )}
            <button
              type="button"
              className="ml-auto text-ui-muted-foreground hover:text-ui-foreground"
              onClick={() => setMoves((m) => m.filter((x) => x.move !== move))}
            >
              {T.result.dismiss}
            </button>
          </div>
        ))}
        <ContextMenu.Root>
          <ContextMenu.Trigger asChild>
            <div
              ref={triggerRef}
              className="flex min-h-0 flex-1"
              onContextMenu={() => {
                if (pendingDrop.current) {
                  setMenu(pendingDrop.current);
                  pendingDrop.current = undefined;
                }
              }}
            >
              {palette && (
                <LogList
                  model={model}
                  palette={palette}
                  font={font}
                  selected={selectedSet}
                  onSelect={(index, mode) =>
                    setSelection((s) => selectRows(model.rows, s.selected, s.anchor, index, mode))
                  }
                  onOpen={(row) => openCommit?.(row.hash)}
                  onCopy={() => copyHashes(selected)}
                  onContextRow={(row, ref) => {
                    if (!pendingDrop.current) setMenu({ kind: "row", row, ref });
                  }}
                  onDropLabel={onDropLabel}
                  onDrawn={onDrawn}
                  reveal={reveal}
                  label={T.list}
                  describe={(row) => `${short(row.hash)} ${row.subject} — ${row.authorName}`}
                  formatDate={formatDate}
                />
              )}
            </div>
          </ContextMenu.Trigger>
          <ContextMenu.Portal>
            <ContextMenu.Content
              data-testid="git-log-menu"
              className="min-w-[220px] rounded-md border border-ui-border bg-ui-panel py-1 text-[12px] text-ui-foreground shadow-lg"
            >
              {menu?.kind === "row" && ops && (
                <RowMenu
                  target={menu}
                  selected={selected}
                  ops={ops}
                  strings={T}
                  isMerge={(h) => (model.rows[model.indexOf(h)]?.parents.length ?? 0) > 1}
                  onOpen={openCommit}
                  onCompare={compare}
                  onCopy={copyHashes}
                />
              )}
              {menu?.kind === "drop" && ops && (
                <DropMenu target={menu} refs={refs} ops={ops} strings={T} onCompare={compare} />
              )}
            </ContextMenu.Content>
          </ContextMenu.Portal>
        </ContextMenu.Root>
        <div
          data-testid="git-log-status"
          className="border-t border-ui-border px-2 py-0.5 text-[11px] text-ui-muted-foreground"
        >
          {status}
          {model.filtered && ` · ${T.filter.noGraph}`}
        </div>
      </div>
      <div className="w-[clamp(160px,28%,360px)] shrink-0 overflow-auto border-l border-ui-border">
        <CommitDetail
          source={source}
          selected={selected}
          strings={T.detail}
          formatDate={formatDate}
          onSelectCommit={selectHash}
          onOpenFile={openRevisionFile}
          onOpenCommit={openCommit}
          onCompare={compare}
        />
      </div>
      {dialogs.element}
      {gitDialogs.element}
    </div>
  );
}

function RowMenu({
  target,
  selected,
  ops,
  strings: T,
  isMerge,
  onOpen,
  onCompare,
  onCopy,
}: {
  target: Extract<MenuTarget, { kind: "row" }>;
  selected: readonly string[];
  ops: LogOps;
  strings: ReturnType<typeof useGitLogStrings>;
  isMerge(hash: string): boolean;
  /** 없으면 그 항목을 두지 않는다(M2-15 의 액션이 이 창에 없다). */
  onOpen?(rev: string): void;
  onCompare?(a: string, b: string): void;
  onCopy(hashes: readonly string[]): void;
}) {
  const { row } = target;
  const hash = row.hash;
  // 메뉴는 고른 것 전체에 건다(우클릭한 행이 고른 것 밖이면 그 행만 — log-list 가 먼저 고른다).
  const commits = selected.includes(hash) ? selected : [hash];
  const many = commits.length > 1;
  const branches = row.refs.filter((r) => r.kind === "branch" && !r.current);
  const remotes = row.refs.filter((r) => r.kind === "remote");
  return (
    <>
      {branches.map((r) => (
        <MenuItem
          key={r.name}
          onSelect={() => void ops.checkoutBranch(r.name, T.menu.checkoutBranch(r.name))}
        >
          {T.menu.checkoutBranch(r.name)}
        </MenuItem>
      ))}
      {remotes.map((r) => (
        <MenuItem
          key={r.name}
          onSelect={() => void ops.checkoutRemote(r.name, T.menu.checkoutBranch(r.name))}
        >
          {T.menu.checkoutBranch(r.name)}
        </MenuItem>
      ))}
      <MenuItem
        disabled={many}
        onSelect={() => void ops.checkoutCommit(hash, T.menu.checkoutRevision)}
      >
        {T.menu.checkoutRevision}
      </MenuItem>
      {SEPARATOR}
      <MenuItem disabled={many} onSelect={() => void ops.createBranch(hash, T.menu.newBranch)}>
        {T.menu.newBranch}
      </MenuItem>
      <MenuItem disabled={many} onSelect={() => void ops.createTag(hash, T.menu.newTag)}>
        {T.menu.newTag}
      </MenuItem>
      {SEPARATOR}
      <MenuItem
        // 머지 커밋은 부모를 고르지 않으면 cherry-pick 할 수 없다(git 이 거절한다).
        disabled={commits.some(isMerge)}
        onSelect={() => void ops.cherryPick(commits, T.menu.cherryPick(commits.length))}
      >
        {T.menu.cherryPick(commits.length)}
      </MenuItem>
      {/* 머지 커밋은 어느 부모 쪽으로 되돌릴지(`-m`)를 골라야 한다 — 아직 없다(13 「하지 않는 것」). */}
      <MenuItem
        disabled={commits.some(isMerge)}
        onSelect={() => void ops.revert(commits, T.menu.revert(commits.length))}
      >
        {T.menu.revert(commits.length)}
      </MenuItem>
      <ContextMenu.Sub>
        <ContextMenu.SubTrigger
          disabled={many}
          className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
        >
          {T.menu.reset} ▸
        </ContextMenu.SubTrigger>
        <ContextMenu.Portal>
          <ContextMenu.SubContent className="min-w-[200px] rounded-md border border-ui-border bg-ui-panel py-1 text-[12px] text-ui-foreground shadow-lg">
            {RESET_MODES.map((mode) => (
              <MenuItem
                key={mode}
                onSelect={() => void ops.reset(hash, mode, T.menu.resetMode(mode))}
              >
                {T.menu.resetMode(mode)}
              </MenuItem>
            ))}
          </ContextMenu.SubContent>
        </ContextMenu.Portal>
      </ContextMenu.Sub>
      {SEPARATOR}
      <MenuItem onSelect={() => onCopy(commits)}>{T.menu.copyHash}</MenuItem>
      {onOpen && (
        <MenuItem disabled={many} onSelect={() => onOpen(hash)}>
          {T.menu.showCommit}
        </MenuItem>
      )}
      {onCompare && (
        <MenuItem
          disabled={commits.length !== 2}
          onSelect={() => onCompare(commits[1]!, commits[0]!)}
        >
          {T.menu.compare}
        </MenuItem>
      )}
    </>
  );
}

function DropMenu({
  target,
  refs,
  ops,
  strings: T,
  onCompare,
}: {
  target: Extract<MenuTarget, { kind: "drop" }>;
  refs: GitRefs | undefined;
  ops: LogOps;
  strings: ReturnType<typeof useGitLogStrings>;
  onCompare?(a: string, b: string): void;
}) {
  // 메뉴에 보이는 것은 짧은 이름, git 에 넘기는 것은 ref 전체 이름(log-ops.ts 머리 주석).
  const source = target.source.name;
  const sourceRev = refFullName(target.source);
  // 놓은 자리의 이름: 라벨이면 그 이름, 행이면 그 커밋.
  const destRef = target.target.ref;
  const dest = destRef?.name ?? short(target.target.row.hash);
  const destRev = destRef ? refFullName(destRef) : target.target.row.hash;
  const current = refs?.head?.kind === "branch" ? refs.head.name : undefined;
  // merge 는 지금 브랜치로만 들여온다(`git.merge` — HEAD 에). 놓은 자리가 지금 브랜치(또는 HEAD 커밋)일 때만.
  const intoCurrent =
    current !== undefined &&
    ((destRef?.kind === "branch" && destRef.name === current) ||
      (target.target.ref === undefined &&
        refs?.head?.kind === "branch" &&
        refs.head.commit === target.target.row.hash));
  const sourceIsLocal = target.source.kind === "branch";
  return (
    <>
      <MenuItem
        disabled={!intoCurrent}
        onSelect={() => void ops.merge(sourceRev, T.menu.merge(source, dest))}
      >
        {intoCurrent ? T.menu.merge(source, dest) : T.menu.mergeNeedsCheckout(dest)}
      </MenuItem>
      <MenuItem
        disabled={!sourceIsLocal}
        onSelect={() => void ops.rebase(source, destRev, T.menu.rebase(source, dest))}
      >
        {T.menu.rebase(source, dest)}
      </MenuItem>
      {onCompare && (
        <MenuItem onSelect={() => onCompare(destRev, sourceRev)}>
          {T.menu.compareRefs(dest, source)}
        </MenuItem>
      )}
    </>
  );
}
