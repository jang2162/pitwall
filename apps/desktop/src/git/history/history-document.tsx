// 파일 히스토리 문서(F5-16, 13-m2-plan M2-18 — 10-ui.md 「에디터 영역 문서 타입」 의 파일 히스토리 행): 에디터 탭 하나에 위는 그
// 파일의 커밋 목록, 아래는 고른 커밋에서의 그 파일 diff.
//
// - 목록은 `git.fileHistory`(M2-05 — `git log --follow` 와 같은 커밋 · 차례 · 그때 경로)이고 HEAD 에서 시작한다. HEAD 가 움직이면
//   (`git.refsChanged`, 이 창의 `git.watch` 가 낸다) 다시 받는다 — 고른 커밋은 해시로 남는다. 보이는 행만 그린다(TanStack Virtual).
// - 행: 바뀜 글자(A · M · D · R · C · T, 색 + 글자) · 짧은 해시 · 제목 · 작성자 · 날짜. 이름이 바뀐(복사된) 커밋은 「이름 바뀜 옛 →
//   새」 를 단다(`git.renamed` 색). 그때 경로가 지금 경로와 다른 행은 툴팁에 그 경로를 적는다.
// - diff 는 M2-15 의 diff 뷰(changes-view.tsx)다: 그 커밋과 첫 부모(루트 커밋은 빈 트리)를 **그 커밋에서의 경로**(이름 바뀜이면
//   옛 경로도)로 거른 `git.diff`. 더블클릭 · `Enter` 는 같은 diff 를 탭으로 연다(params.git 의 `diff`).
// - 우클릭: Git 로그에서 선택(`git.log.reveal`) · 커밋 상세(`git.showCommit`) · diff 탭 · 리비전의 파일(`git.openRevisionFile`,
//   지운 커밋은 끈다) · 해시 복사. 다른 행의 액션은 이 창에 있을 때만 둔다(`isAvailable`).

import * as ContextMenu from "@radix-ui/react-context-menu";
import type { FileChangeKind, FileHistoryRow } from "@pitwall/rpc-client";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type ReactNode, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useKeymap, useKeymapPopup } from "../../keymap/keymap-provider";
import { useProjectStrings } from "../../layout/strings";
import { useNotify } from "../../notifications/notifications";
import { useHost } from "../../rpc/host";
import { ChangesView } from "../documents/changes-view";
import { openGitDocument } from "../documents/git-documents";
import type { GitDocumentSpec } from "../documents/spec";
import { HistoryModel, type HistorySource } from "./history-model";
import { type GitHistoryStrings, useGitHistoryStrings } from "./strings";

/** 목록 행 높이(px). */
export const ROW_HEIGHT = 22;

const short = (hash: string) => hash.slice(0, 7);
const baseName = (path: string) => path.split("/").pop() ?? path;

const CHANGE: Record<FileChangeKind, { letter: string; tone: string }> = {
  added: { letter: "A", tone: "text-git-added" },
  modified: { letter: "M", tone: "text-git-modified" },
  deleted: { letter: "D", tone: "text-git-deleted" },
  renamed: { letter: "R", tone: "text-git-renamed" },
  copied: { letter: "C", tone: "text-git-added" },
  typeChanged: { letter: "T", tone: "text-git-modified" },
};

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });
const formatDate = (seconds: number) => dateFormat.format(new Date(seconds * 1000));

/** 그 커밋의 그 파일 diff(params.git 의 `diff` — 첫 부모와, 그때 경로로). */
export function rowDiffSpec(row: FileHistoryRow): GitDocumentSpec {
  return {
    kind: "diff",
    to: { kind: "commit", rev: row.hash },
    path: row.path,
    ...(row.oldPath !== null && { oldPath: row.oldPath }),
  };
}

/** 그 커밋에서 `git.diff` 를 거를 경로: 그때 경로와 이름 바뀜 · 복사의 원래 경로. */
export function rowPaths(row: FileHistoryRow): string[] {
  return row.oldPath !== null ? [row.path, row.oldPath] : [row.path];
}

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

function RenameNote({ row, strings: T }: { row: FileHistoryRow; strings: GitHistoryStrings }) {
  if (row.oldPath === null) return null;
  const text =
    row.change === "copied" ? T.copied(row.oldPath, row.path) : T.renamed(row.oldPath, row.path);
  return (
    <span data-git-history-rename="" className="min-w-0 shrink truncate text-git-renamed">
      {text}
    </span>
  );
}

function HistoryRow({
  row,
  selected,
  current,
  start,
  strings: T,
  onSelect,
  onOpen,
}: {
  row: FileHistoryRow;
  selected: boolean;
  /** 지금(목록 맨 위) 경로. 다르면 툴팁에 그때 경로를 적는다. */
  current: string | undefined;
  start: number;
  strings: GitHistoryStrings;
  onSelect(row: FileHistoryRow): void;
  onOpen(row: FileHistoryRow): void;
}) {
  const change = CHANGE[row.change];
  return (
    <div
      id={`git-history-${row.hash}`}
      role="option"
      aria-selected={selected}
      data-git-history-row={row.hash}
      data-path={row.path}
      title={row.path !== current ? T.pathAt(row.path) : undefined}
      onPointerDown={(e) => {
        if (e.button === 0 || e.button === 2) onSelect(row);
      }}
      onDoubleClick={() => onOpen(row)}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        right: 0,
        height: ROW_HEIGHT,
        transform: `translateY(${start}px)`,
      }}
      className={`flex cursor-default items-center gap-2 px-2 text-[12px] ${
        selected ? "bg-ui-active" : "hover:bg-ui-hover"
      }`}
    >
      <span
        role="img"
        aria-label={T.change[row.change]}
        title={T.change[row.change]}
        className={`w-3 flex-none text-center font-mono font-semibold ${change.tone}`}
      >
        {change.letter}
      </span>
      <span className="flex-none font-mono text-ui-muted-foreground">{short(row.hash)}</span>
      <span className="min-w-0 flex-1 truncate">{row.subject}</span>
      <RenameNote row={row} strings={T} />
      <span className="w-[120px] flex-none truncate text-ui-muted-foreground">
        {row.authorName}
      </span>
      <span className="w-[110px] flex-none truncate text-right text-ui-muted-foreground">
        {formatDate(row.authorTime)}
      </span>
    </div>
  );
}

/** diff 칸 위의 머리: 고른 커밋의 제목 · 해시 · 작성자 · 그때 경로. */
function CommitLine({ row, strings: T }: { row: FileHistoryRow; strings: GitHistoryStrings }) {
  return (
    <div
      data-git-history-commit={row.hash}
      className="flex flex-none flex-wrap items-center gap-x-3 border-b border-ui-border px-3 py-1 text-[12px]"
    >
      <span className="font-semibold text-ui-foreground">{row.subject}</span>
      <span className="font-mono text-ui-muted-foreground select-text">{short(row.hash)}</span>
      <span className="text-ui-muted-foreground">
        {row.authorName} · {formatDate(row.authorTime)}
      </span>
      <span className="font-mono text-ui-muted-foreground">{row.path}</span>
      <RenameNote row={row} strings={T} />
    </div>
  );
}

export function HistoryDocument({
  path,
  worktree,
}: {
  path: string;
  worktree: string | undefined;
}) {
  const T = useGitHistoryStrings();
  const D = useProjectStrings().git.documents;
  const { client, target } = useHost();
  const projectId = "projectId" in target ? target.projectId : undefined;
  const { registry } = useKeymap();
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const [model] = useState(() => new HistoryModel());
  useSyncExternalStore(model.subscribe, model.getVersion);
  // 액션이 등록 · 해제되면 다시 그린다(메뉴 항목을 둘지가 바뀐다).
  useSyncExternalStore(registry.subscribe, registry.getVersion);
  const [chosen, setChosen] = useState<string>();
  const [menuRow, setMenuRow] = useState<FileHistoryRow>();
  const [menuOpen, setMenuOpen] = useState(false);
  useKeymapPopup(menuOpen);

  const source: HistorySource | undefined = useMemo(
    () => (client && projectId ? { client, projectId, worktree } : undefined),
    [client, projectId, worktree],
  );

  useEffect(() => () => model.dispose(), [model]);
  useEffect(() => {
    if (source) void model.load(source, path);
  }, [model, source, path]);

  // HEAD 가 움직이면(커밋 · 체크아웃 · reset) 다시 받는다.
  useEffect(() => {
    if (!source) return;
    let stopped = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    void source.client
      .subscribe({ projectId: source.projectId, topics: ["git.refsChanged"] }, (event) => {
        const changed = event.payload as { worktree?: string | null } | null;
        if (stopped || (changed?.worktree ?? undefined) !== source.worktree) return;
        void model.load(source, path);
      })
      .then(
        (s) => {
          unsubscribe = () => s.unsubscribe();
          if (stopped) void unsubscribe().catch(() => {});
        },
        (e: unknown) => console.warn("[git] file history could not follow ref changes", e),
      );
    return () => {
      stopped = true;
      void unsubscribe?.().catch(() => {});
    };
  }, [model, source, path]);

  const rows = model.rows;
  // 고른 커밋은 해시로 든다. 목록에 없으면(처음 · 다시 받았더니 빠짐) 맨 위 행.
  const index = Math.max(
    0,
    rows.findIndex((r) => r.hash === chosen),
  );
  const row = rows[index];

  const scroller = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
    // 시험 환경(크기를 재지 못함)에서도 앞 행들이 그려지게 처음 높이를 준다.
    initialRect: { width: 800, height: 300 },
  });

  const select = (next: number) => {
    const r = rows[Math.min(rows.length - 1, Math.max(0, next))];
    if (!r) return;
    setChosen(r.hash);
    virtual.scrollToIndex(rows.indexOf(r));
  };
  const openDiff = (r: FileHistoryRow) =>
    openGitDocument(
      workspace,
      rowDiffSpec(r),
      D.revisionTitle(baseName(r.path), short(r.hash)),
      worktree,
    );
  const copyHash = (hash: string) =>
    void navigator.clipboard.writeText(hash).then(
      () => notify({ level: "info", title: T.copiedHash }),
      (e: unknown) => console.warn("[git] could not copy", e),
    );

  const page = () => Math.max(1, Math.floor((scroller.current?.clientHeight ?? 0) / ROW_HEIGHT));
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") select(index + 1);
    else if (e.key === "ArrowUp") select(index - 1);
    else if (e.key === "PageDown") select(index + page());
    else if (e.key === "PageUp") select(index - page());
    else if (e.key === "Home") select(0);
    else if (e.key === "End") select(rows.length - 1);
    else if (e.key === "Enter" && row) openDiff(row);
    else return;
    e.preventDefault();
  };

  const failure =
    model.status.kind === "error"
      ? model.status.submodule !== undefined
        ? T.submodule(model.status.submodule)
        : T.failed(model.status.message)
      : undefined;
  const status =
    failure !== undefined
      ? failure
      : model.status.kind === "loading" && rows.length === 0
        ? T.loading
        : model.status.kind === "done" && rows.length === 0
          ? T.empty
          : T.count(rows.length);

  const current = rows[0]?.path;
  return (
    <div data-git-history={path} className="flex h-full min-h-0 flex-col bg-editor-background">
      <div className="flex flex-none items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px] text-ui-muted-foreground">
        <span className="min-w-0 truncate font-mono" title={path}>
          {path}
        </span>
        <span data-git-history-status="" role="status" className="ml-auto flex-none">
          {status}
        </span>
      </div>
      <ContextMenu.Root onOpenChange={setMenuOpen}>
        <ContextMenu.Trigger asChild>
          <div
            ref={scroller}
            role="listbox"
            aria-label={T.list}
            aria-activedescendant={row ? `git-history-${row.hash}` : undefined}
            tabIndex={0}
            data-git-history-list=""
            onKeyDown={onKeyDown}
            className="h-[38%] min-h-[120px] flex-none overflow-auto border-b border-ui-border outline-none focus-visible:ring-1 focus-visible:ring-ui-accent"
          >
            <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
              {virtual.getVirtualItems().map((item) => {
                const r = rows[item.index];
                if (!r) return null;
                return (
                  <HistoryRow
                    key={r.hash}
                    row={r}
                    selected={r === row}
                    current={current}
                    start={item.start}
                    strings={T}
                    onSelect={(x) => {
                      setChosen(x.hash);
                      setMenuRow(x);
                    }}
                    onOpen={openDiff}
                  />
                );
              })}
            </div>
          </div>
        </ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenu.Content
            data-testid="git-history-menu"
            className="min-w-[220px] rounded-md border border-ui-border bg-ui-panel py-1 text-[12px] text-ui-foreground shadow-lg"
          >
            {menuRow && (
              <>
                {registry.isAvailable("git.log.reveal") && (
                  <MenuItem onSelect={() => registry.run("git.log.reveal", { rev: menuRow.hash })}>
                    {T.rowMenu.reveal}
                  </MenuItem>
                )}
                {registry.isAvailable("git.showCommit") && (
                  <MenuItem onSelect={() => registry.run("git.showCommit", { rev: menuRow.hash })}>
                    {T.rowMenu.showCommit}
                  </MenuItem>
                )}
                <MenuItem onSelect={() => openDiff(menuRow)}>{T.rowMenu.openDiff}</MenuItem>
                {registry.isAvailable("git.openRevisionFile") && (
                  <MenuItem
                    disabled={menuRow.change === "deleted"}
                    onSelect={() =>
                      registry.run("git.openRevisionFile", {
                        rev: menuRow.hash,
                        path: menuRow.path,
                      })
                    }
                  >
                    {T.rowMenu.openRevision}
                  </MenuItem>
                )}
                <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
                <MenuItem onSelect={() => copyHash(menuRow.hash)}>{T.rowMenu.copyHash}</MenuItem>
              </>
            )}
          </ContextMenu.Content>
        </ContextMenu.Portal>
      </ContextMenu.Root>
      <div className="min-h-0 flex-1">
        {row ? (
          <ChangesView
            key={row.hash}
            from={undefined}
            to={{ kind: "commit", rev: row.hash }}
            worktree={worktree}
            paths={rowPaths(row)}
            tree={false}
            header={<CommitLine row={row} strings={T} />}
          />
        ) : (
          <div
            role="status"
            className="flex h-full items-center justify-center px-4 text-center text-ui-muted-foreground"
          >
            {failure ?? (model.status.kind === "done" ? T.empty : T.pick)}
          </div>
        )}
      </div>
    </div>
  );
}
