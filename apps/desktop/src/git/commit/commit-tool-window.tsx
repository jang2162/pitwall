// 커밋 툴 윈도우(⌘0 — docs/design/13-m2-plan.md M2-16, 10-ui.md 「주요 툴 윈도우 상세」 의 커밋 그림): 위 막대 · 진행 중 작업 배너
// 자리 · 그룹 목록(TanStack Virtual) · 커밋 메시지 칸 · 훅 출력.
//
// - 목록은 창의 git 상태(`windowGitStatus()` — M2-12 의 감시 하나)를 읽는다. 감시를 더 열지 않는다 — 터미널 · 에이전트의 `git add` 가
//   `git.statusChanged` 로 오면 그 저장소가 바뀌고 그룹이 다시 묶인다.
// - 파일을 고르면 에디터 영역에 diff 미리보기 탭(스테이징됨 = HEAD ↔ index, 변경 = index ↔ 작업 트리 — 그 탭의 hunk · 줄 체크가
//   부분 스테이징이다, staging-changes.tsx). 버전 관리 안 되는 파일은 파일 자체를 미리보기로 연다. 더블클릭 · Enter 는 고정 탭.
// - 체크 = 스테이징(commit-model.ts). 쓰기는 commit-ops.ts.
// - 진행 중 작업 배너(M2-19 의 `OperationBanner`, `placement="commit"`)를 위 막대와 목록 사이에 단다. 배너는 창의 키맵(액션)을 쓰므로
//   툴 윈도우가 넘긴다 — 키맵 없이 목록만 그리는 측정 하네스는 넘기지 않는다.
// - stash 는 목록 끝의 그룹이다: 만들기(메시지를 묻는다, untracked 포함 체크) · 적용 · 꺼내기 · 버리기(확인).

import type { StashEntry } from "@pitwall/rpc-client";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IDockviewPanelProps } from "dockview-react";
import { ChevronDown, ChevronRight, FileDiff, History, RefreshCw, Undo2 } from "lucide-react";
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useBranch } from "../branch-state";
import { openGitDocument } from "../documents/git-documents";
import type { GitDocumentSpec } from "../documents/spec";
import { activeWorktree } from "../../open-with/active-worktree";
import { windowGitStatus } from "../git-status";
import { gitToneClass } from "../git-status-mark";
import { OperationBanner } from "../merge/operation-banner";
import {
  type CheckState,
  type CommitGroup,
  type CommitGroups,
  type CommitItem,
  type CommitRow,
  commitRows,
  GROUPS,
  groupCheck,
  groupEntries,
  itemCheck,
  selectItems,
} from "./commit-model";
import {
  commitFeature,
  commitPanel,
  type CommitFeature,
  reloadStashes,
  runCommit,
  setAmend,
  setMessage,
} from "./commit-state";
import { type GitCommitStrings, useGitCommitStrings } from "./strings";

/** 목록 한 행의 높이(px). */
export const ROW = 22;

/** 창의 git 상태의 그룹. 상태가 바뀔 때만 다시 묶는다. */
export function useCommitGroups(): {
  groups: CommitGroups;
  workTree: string | null;
  unsupported: boolean;
} {
  const store = windowGitStatus();
  const version = useSyncExternalStore(
    (l) => store.subscribe(l),
    () => store.getVersion(),
  );
  return useMemo(
    () => ({
      groups: groupEntries(store.entries()),
      workTree: store.workTree(),
      unsupported: store.unsupported() !== null,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 저장소가 바뀌면 version 이 오른다.
    [store, version],
  );
}

/** 고른 키의 항목(목록 차례). */
export function itemsOf(groups: CommitGroups, keys: readonly string[]): CommitItem[] {
  const wanted = new Set(keys);
  return GROUPS.flatMap((g) => groups[g].filter((i) => wanted.has(i.key)));
}

/** 항목 하나의 diff 미리보기 문서(충돌 · untracked 는 없다). */
export function previewSpec(item: CommitItem): GitDocumentSpec | undefined {
  if (item.group === "staged")
    return {
      kind: "diff",
      to: { kind: "index" },
      path: item.path,
      ...(item.origPath && { oldPath: item.origPath }),
    };
  if (item.group === "changes") return { kind: "diff", to: { kind: "workTree" }, path: item.path };
  return undefined;
}

function Checkbox({
  state,
  label,
  disabled,
  onToggle,
}: {
  state: CheckState;
  label: string;
  disabled?: boolean;
  onToggle(): void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = state === "mixed";
  }, [state]);
  if (state === "none") return <span className="w-[13px] flex-none" />;
  return (
    <input
      ref={ref}
      type="checkbox"
      aria-label={label}
      title={label}
      data-commit-check={state}
      checked={state === "checked"}
      disabled={disabled}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => {
        // 체크는 다시 읽은 상태가 정한다 — 누른 때 바꾸지 않는다.
        e.preventDefault();
        onToggle();
      }}
      className="h-[13px] w-[13px] flex-none"
    />
  );
}

function ToolButton({
  label,
  onClick,
  disabled,
  children,
}: {
  label: string;
  onClick(): void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="flex h-[22px] w-[22px] items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover disabled:opacity-40"
    >
      {children}
    </button>
  );
}

const GROUP_NAME: Record<CommitGroup, (T: GitCommitStrings) => string> = {
  conflicts: (T) => T.groups.conflicts,
  staged: (T) => T.groups.staged,
  changes: (T) => T.groups.changes,
  untracked: (T) => T.groups.untracked,
};

export function CommitToolWindow({ api }: IDockviewPanelProps) {
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      <CommitView banner={<OperationBanner placement="commit" />} />
    </div>
  );
}

export interface CommitViewProps {
  /** 위 막대 아래에 둘 진행 중 작업 배너. */
  banner?: ReactNode;
  /** 성능 측정: 그룹 행을 그린 뒤 부른다. */
  onDrawn?(): void;
}

export function CommitView({ banner, onDrawn }: CommitViewProps) {
  const T = useGitCommitStrings();
  const feature = commitFeature((s) => s.current);
  const { groups, workTree, unsupported } = useCommitGroups();
  if (!feature) return <Centered text={T.list.noProject} />;
  return (
    <CommitBody
      feature={feature}
      groups={groups}
      workTree={workTree}
      unsupported={unsupported}
      banner={banner}
      onDrawn={onDrawn}
    />
  );
}

function Centered({ text }: { text: string }) {
  return (
    <div className="flex h-full items-center justify-center px-3 text-center text-ui-muted-foreground">
      {text}
    </div>
  );
}

function CommitBody({
  feature,
  groups,
  workTree,
  unsupported,
  banner,
  onDrawn,
}: {
  feature: CommitFeature;
  groups: CommitGroups;
  workTree: string | null;
  unsupported: boolean;
  banner?: ReactNode;
  onDrawn?(): void;
}) {
  const T = useGitCommitStrings();
  const workspace = useEditorWorkspaceOptional();
  const panel = commitPanel();
  const { refs } = useBranch();
  const rows = useMemo(
    () => commitRows(groups, panel.collapsed, panel.stashes),
    [groups, panel.collapsed, panel.stashes],
  );
  const [writing, setWriting] = useState(false);

  // stash 목록: 처음과 ref 가 바뀔 때(`refs/stash` 도 ref 다 — 터미널의 `git stash` 를 따른다).
  useEffect(() => {
    void reloadStashes(feature);
  }, [feature, refs]);

  // 고른 것 중 사라진 것을 뺀다(스테이징 · 커밋으로 그룹이 바뀜).
  useEffect(() => {
    const { selected } = commitPanel.getState();
    if (selected.length === 0) return;
    const alive = new Set(itemsOf(groups, selected).map((i) => i.key));
    if (alive.size !== selected.length)
      commitPanel.setState({ selected: selected.filter((k) => alive.has(k)) });
  }, [groups]);

  const write = async (job: () => Promise<unknown>) => {
    if (writing) return;
    setWriting(true);
    try {
      await job();
    } finally {
      setWriting(false);
    }
  };

  const open = (item: CommitItem, preview: boolean) => {
    if (!workspace) return;
    const worktree = activeWorktree();
    const spec = previewSpec(item);
    if (spec) {
      openGitDocument(workspace, spec, item.path.split("/").pop() ?? item.path, worktree, preview);
      return;
    }
    if (item.group === "untracked" && workTree !== null)
      void workspace.open({ path: `${workTree}/${item.path}`, worktree, preview });
  };

  const select = (item: CommitItem, mode: "single" | "toggle" | "range") => {
    const s = commitPanel.getState();
    const next = selectItems(rows, s.selected, s.anchor, item.key, mode);
    commitPanel.setState(next);
    if (mode === "single") open(item, true);
  };

  const toggleCollapsed = (key: string) => {
    const next = new Set(commitPanel.getState().collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    commitPanel.setState({ collapsed: next });
  };

  const selectedItems = () => itemsOf(groups, commitPanel.getState().selected);

  let list: ReactNode;
  if (unsupported) list = <Centered text={T.list.unsupported} />;
  else if (workTree === null) list = <Centered text={T.list.noRepository} />;
  else
    list = (
      <CommitList
        rows={rows}
        groups={groups}
        selected={panel.selected}
        disabled={writing}
        onSelect={select}
        onOpen={(item) => open(item, false)}
        onToggleGroup={(g) =>
          write(() => feature.ops.toggle(groups[g].filter((i) => itemCheck(i) !== "none")))
        }
        onToggleItem={(item) => write(() => feature.ops.toggle([item]))}
        onCollapse={toggleCollapsed}
        onStash={(stash, how) =>
          write(async () => {
            const done =
              how === "drop"
                ? await feature.ops.stashDrop(stash)
                : await feature.ops.stashApply(stash, how === "pop");
            if (done || how !== "apply") await reloadStashes(feature);
          })
        }
        onStashCreate={() =>
          write(async () => {
            if (await feature.ops.stashPush(commitPanel.getState().includeUntracked))
              await reloadStashes(feature);
          })
        }
        onDrawn={onDrawn}
      />
    );

  return (
    <>
      <div className="flex flex-none items-center gap-0.5 border-b border-ui-border px-1 py-0.5">
        <ToolButton
          label={T.toolbar.refresh}
          onClick={() =>
            void write(async () => {
              const status = await feature.ops.status().catch(() => undefined);
              if (status) windowGitStatus().apply(status);
              await reloadStashes(feature);
            })
          }
        >
          <RefreshCw size={14} aria-hidden />
        </ToolButton>
        <ToolButton
          label={T.toolbar.rollback}
          disabled={writing || panel.selected.length === 0}
          onClick={() => void write(() => feature.ops.rollback(selectedItems()))}
        >
          <Undo2 size={14} aria-hidden />
        </ToolButton>
        <ToolButton
          label={T.toolbar.showDiff}
          disabled={panel.selected.length === 0}
          onClick={() => {
            const first = selectedItems()[0];
            if (first) open(first, false);
          }}
        >
          <FileDiff size={14} aria-hidden />
        </ToolButton>
      </div>
      {banner}
      <div className="min-h-0 flex-1">{list}</div>
      <MessageArea feature={feature} />
    </>
  );
}

/** 툴 윈도우는 프로젝트 창의 에디터 작업 공간 안에서 그려진다. 측정 하네스처럼 밖이면 없다. */
function useEditorWorkspaceOptional() {
  try {
    return useEditorWorkspace();
  } catch {
    return undefined;
  }
}

function CommitList({
  rows,
  groups,
  selected,
  disabled,
  onSelect,
  onOpen,
  onToggleGroup,
  onToggleItem,
  onCollapse,
  onStash,
  onStashCreate,
  onDrawn,
}: {
  rows: readonly CommitRow[];
  groups: CommitGroups;
  selected: readonly string[];
  disabled: boolean;
  onSelect(item: CommitItem, mode: "single" | "toggle" | "range"): void;
  onOpen(item: CommitItem): void;
  onToggleGroup(group: CommitGroup): void;
  onToggleItem(item: CommitItem): void;
  onCollapse(key: string): void;
  onStash(stash: StashEntry, how: "apply" | "pop" | "drop"): void;
  onStashCreate(): void;
  onDrawn?(): void;
}) {
  const T = useGitCommitStrings();
  const collapsed = commitPanel((s) => s.collapsed);
  const includeUntracked = commitPanel((s) => s.includeUntracked);
  const scroller = useRef<HTMLDivElement>(null);
  const chosen = useMemo(() => new Set(selected), [selected]);
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 12,
    // 시험 환경(크기를 재지 못함)에서도 앞 행들이 그려지게 처음 높이를 준다.
    initialRect: { width: 320, height: 600 },
  });
  useEffect(() => onDrawn?.());

  const fileRows = rows.flatMap((r) => (r.kind === "file" ? [r.item] : []));
  const onKeyDown = (e: KeyboardEvent) => {
    const current = fileRows.findIndex((i) => i.key === selected.at(-1));
    const move = (to: number) => {
      const item = fileRows[Math.min(fileRows.length - 1, Math.max(0, to))];
      if (!item) return;
      onSelect(item, e.shiftKey ? "range" : "single");
      const index = rows.findIndex((r) => r.kind === "file" && r.item.key === item.key);
      virtual.scrollToIndex(index);
    };
    if (e.key === "ArrowDown") move(current + 1);
    else if (e.key === "ArrowUp") move(current < 0 ? 0 : current - 1);
    else if (e.key === "Enter" && current >= 0) onOpen(fileRows[current]!);
    else if (e.key === " " && current >= 0) {
      const item = fileRows[current]!;
      if (itemCheck(item) !== "none") onToggleItem(item);
    } else return;
    e.preventDefault();
  };

  const hasFiles = GROUPS.some((g) => groups[g].length > 0);
  return (
    <div
      ref={scroller}
      role="tree"
      tabIndex={0}
      aria-label={T.title}
      data-commit-list=""
      onKeyDown={onKeyDown}
      className="h-full overflow-auto text-[13px] outline-none"
    >
      {!hasFiles && (
        <div className="px-3 py-2 text-ui-muted-foreground" data-commit-empty="">
          {T.list.noChanges}
        </div>
      )}
      <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
        {virtual.getVirtualItems().map((v) => {
          const row = rows[v.index]!;
          const style = {
            position: "absolute" as const,
            top: 0,
            left: 0,
            right: 0,
            height: ROW,
            transform: `translateY(${v.start}px)`,
          };
          if (row.kind === "group") {
            const open = !collapsed.has(row.group);
            const check = groupCheck(row.group, groups[row.group]);
            return (
              <div
                key={`g:${row.group}`}
                role="treeitem"
                aria-expanded={open}
                data-commit-group={row.group}
                style={style}
                onClick={() => onCollapse(row.group)}
                className="flex cursor-default items-center gap-1 px-1 font-semibold hover:bg-ui-hover"
              >
                {open ? (
                  <ChevronDown size={12} aria-hidden />
                ) : (
                  <ChevronRight size={12} aria-hidden />
                )}
                <Checkbox
                  state={check}
                  disabled={disabled}
                  label={row.group === "staged" ? T.list.unstageAll : T.list.stageAll}
                  onToggle={() => onToggleGroup(row.group)}
                />
                <span className="truncate">
                  {T.groups.count(GROUP_NAME[row.group](T), row.count)}
                </span>
              </div>
            );
          }
          if (row.kind === "file") {
            const item = row.item;
            const active = chosen.has(item.key);
            const slash = item.path.lastIndexOf("/");
            const name = item.path.slice(slash + 1);
            const dir = slash > 0 ? item.path.slice(0, slash) : "";
            return (
              <div
                key={item.key}
                role="treeitem"
                aria-selected={active}
                data-commit-item={item.key}
                data-commit-path={item.path}
                title={item.origPath ? `${item.origPath} → ${item.path}` : item.path}
                style={{ ...style, paddingLeft: 20 }}
                onClick={(e) =>
                  onSelect(
                    item,
                    e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "single",
                  )
                }
                onDoubleClick={() => onOpen(item)}
                className={`flex cursor-default items-center gap-1.5 pr-2 ${active ? "bg-ui-active" : "hover:bg-ui-hover"}`}
              >
                <Checkbox
                  state={itemCheck(item)}
                  disabled={disabled}
                  label={item.group === "staged" ? T.list.unstage : T.list.stage}
                  onToggle={() => onToggleItem(item)}
                />
                <span className={`truncate ${gitToneClass({ tone: item.tone })}`}>{name}</span>
                {dir && (
                  <span className="truncate text-[12px] text-ui-muted-foreground">{dir}</span>
                )}
                {item.partial && (
                  <span className="flex-none text-[12px] text-ui-muted-foreground">
                    ({T.list.partial})
                  </span>
                )}
                <span
                  className={`ml-auto flex-none font-mono text-[11px] ${gitToneClass({ tone: item.tone })}`}
                  title={item.group === "conflicts" ? T.list.conflictHint : undefined}
                >
                  {item.letter}
                </span>
              </div>
            );
          }
          if (row.kind === "stashHeader") {
            const open = !collapsed.has("stash");
            return (
              <div
                key="stash"
                role="treeitem"
                aria-expanded={open}
                data-commit-group="stash"
                style={style}
                className="flex cursor-default items-center gap-1 px-1 font-semibold hover:bg-ui-hover"
              >
                <button
                  type="button"
                  onClick={() => onCollapse("stash")}
                  className="flex min-w-0 items-center gap-1"
                >
                  {open ? (
                    <ChevronDown size={12} aria-hidden />
                  ) : (
                    <ChevronRight size={12} aria-hidden />
                  )}
                  <span className="truncate">
                    {row.count > 0 ? T.groups.count(T.groups.stash, row.count) : T.groups.stash}
                  </span>
                </button>
                <label className="ml-auto flex items-center gap-1 text-[12px] font-normal text-ui-muted-foreground">
                  <input
                    type="checkbox"
                    data-commit-stash-untracked=""
                    checked={includeUntracked}
                    onChange={(e) => commitPanel.setState({ includeUntracked: e.target.checked })}
                  />
                  {T.stash.includeUntracked}
                </label>
                <button
                  type="button"
                  data-commit-stash-create=""
                  disabled={disabled}
                  onClick={onStashCreate}
                  className="rounded-sm px-1 text-[12px] font-normal text-ui-accent hover:bg-ui-hover disabled:opacity-40"
                >
                  {T.stash.create}
                </button>
              </div>
            );
          }
          const stash = row.stash;
          return (
            <div
              key={`s:${stash.commit}`}
              role="treeitem"
              data-commit-stash={stash.index}
              title={stash.message}
              style={{ ...style, paddingLeft: 20 }}
              className="group flex cursor-default items-center gap-1.5 pr-1 hover:bg-ui-hover"
            >
              <span className="flex-none font-mono text-[11px] text-ui-muted-foreground">
                {`stash@{${stash.index}}`}
              </span>
              <span className="min-w-0 flex-1 truncate">{stash.message}</span>
              {(["apply", "pop", "drop"] as const).map((how) => (
                <button
                  key={how}
                  type="button"
                  data-commit-stash-action={how}
                  disabled={disabled}
                  onClick={() => onStash(stash, how)}
                  className="flex-none rounded-sm px-1 text-[12px] text-ui-accent hover:bg-ui-active disabled:opacity-40"
                >
                  {T.stash[how]}
                </button>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function MessageArea({ feature }: { feature: CommitFeature }) {
  const T = useGitCommitStrings();
  const message = commitPanel((s) => s.message);
  const amend = commitPanel((s) => s.amend);
  const signoff = commitPanel((s) => s.signoff);
  const busy = commitPanel((s) => s.busy);
  const output = commitPanel((s) => s.output);
  const [history, setHistory] = useState<readonly string[]>();
  return (
    <div className="flex max-h-[60%] flex-none flex-col gap-1 border-t border-ui-border p-1.5">
      {output && (
        <div
          data-commit-output={output.failed ? "failed" : "ok"}
          className={`flex max-h-[160px] min-h-0 flex-col rounded-sm border ${output.failed ? "border-git-deleted" : "border-ui-border"}`}
        >
          <div className="flex flex-none items-center px-1.5 py-0.5 text-[12px]">
            <span className={output.failed ? "text-git-deleted" : "text-ui-muted-foreground"}>
              {output.failed ? T.result.outputFailed : T.result.output}
            </span>
            <button
              type="button"
              onClick={() => commitPanel.setState({ output: undefined })}
              className="ml-auto rounded-sm px-1 text-ui-muted-foreground hover:bg-ui-hover"
            >
              {T.result.close}
            </button>
          </div>
          <pre className="min-h-0 overflow-auto px-1.5 pb-1 font-mono text-[12px] whitespace-pre-wrap select-text">
            {output.text}
          </pre>
        </div>
      )}
      <div className="relative flex items-center text-[12px] text-ui-muted-foreground">
        <span>{T.message.label}</span>
        <button
          type="button"
          title={T.message.history}
          aria-label={T.message.history}
          data-commit-history=""
          onClick={() => setHistory((h) => (h ? undefined : feature.history()))}
          className="ml-auto flex h-[18px] w-[18px] items-center justify-center rounded-sm hover:bg-ui-hover"
        >
          <History size={13} aria-hidden />
        </button>
        {history && (
          <div
            role="listbox"
            data-commit-history-list=""
            onKeyDown={(e) => e.key === "Escape" && setHistory(undefined)}
            className="absolute right-0 bottom-[20px] z-10 max-h-[240px] w-[90%] overflow-auto rounded-sm border border-ui-border bg-ui-panel py-0.5 text-[12px] text-ui-foreground shadow-md"
          >
            {history.length === 0 && (
              <div className="px-2 py-1 text-ui-muted-foreground">{T.message.historyEmpty}</div>
            )}
            {history.map((m, i) => (
              <button
                key={i}
                type="button"
                role="option"
                aria-selected={false}
                title={m}
                onClick={() => {
                  setMessage(m);
                  setHistory(undefined);
                }}
                className="block w-full truncate px-2 py-0.5 text-left hover:bg-ui-hover"
              >
                {m.split("\n")[0]}
              </button>
            ))}
          </div>
        )}
      </div>
      <textarea
        data-commit-message=""
        aria-label={T.message.label}
        placeholder={T.message.label}
        value={message}
        spellCheck={false}
        onChange={(e) => setMessage(e.target.value)}
        className="h-[96px] min-h-[48px] resize-y rounded-sm border border-ui-border bg-editor-background px-1.5 py-1 font-mono text-[12px] text-ui-foreground outline-none focus:border-ui-accent"
      />
      <div className="flex items-center gap-3 text-[12px]">
        <label className="flex items-center gap-1">
          <input
            type="checkbox"
            data-commit-amend=""
            checked={amend}
            onChange={(e) => void setAmend(feature, e.target.checked)}
          />
          {T.message.amend}
        </label>
        <label className="flex items-center gap-1">
          <input
            type="checkbox"
            data-commit-signoff=""
            checked={signoff}
            onChange={(e) => commitPanel.setState({ signoff: e.target.checked })}
          />
          {T.message.signoff}
        </label>
      </div>
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          data-commit-run=""
          disabled={busy}
          onClick={() => void runCommit(feature, false)}
          className="rounded-sm bg-ui-accent px-2.5 py-0.5 text-[12px] text-ui-accent-foreground disabled:opacity-50"
        >
          {busy ? T.message.running : T.message.commit}
        </button>
        <button
          type="button"
          data-commit-run-push=""
          disabled={busy}
          onClick={() => void runCommit(feature, true)}
          className="rounded-sm border border-ui-border px-2.5 py-0.5 text-[12px] hover:bg-ui-hover disabled:opacity-50"
        >
          {T.message.commitAndPush}
        </button>
      </div>
    </div>
  );
}
