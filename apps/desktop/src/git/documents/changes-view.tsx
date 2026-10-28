// 바뀐 파일 트리 + 고른 파일의 diff(커밋 상세 · 두 커밋 비교가 함께 쓴다, 10-ui.md 「에디터 영역 문서 타입」 의 커밋 상세 행).
//
// - 트리: 폴더로 묶고 하나뿐인 하위 폴더는 이어 붙인다(`src/git`, IntelliJ 와 같다). 파일 행은 상태 글자(A · M · D · R · T)와 색,
//   이름이 바뀐 파일은 툴팁에 옛 경로. 처음에는 첫 파일을 고른다. 행이 많아도 보이는 것만 그린다(TanStack Virtual).
// - 파일 diff 의 접기(13-m2-plan M2-04 · F5-3): 생성 파일(`generated` — `Cargo.lock` 등 Linguist 규칙 · `linguist-generated`)은 접힌
//   채로 열고 [펼치기]로 연다(문서마다 기억한다). 너무 큰 파일(`tooLarge`)은 hunk 가 없어 「너무 큼」. 응답 상한에 걸려 hunk 가 빠진
//   파일(`hunksOmitted`)은 고를 때 그 파일만 다시 묻는다. 바이너리 · 서브모듈 포인터 · 읽지 못함은 그 까닭을 보인다.
// - 위 막대: 보기(나란히 / 통합 — 창 안에서 모든 diff 가 함께 쓴다), 공백 무시(이 문서의 `git.diff` 를 `ignoreWhitespace` 로 다시
//   묻는다), 이전 · 다음 차이(`⇧F7` · `F7` 과 같다).
// - 코드 글꼴(`[editor] font_family` · `font_size`)을 diff 뷰에 넘긴다. 확장 지점(`slots`)과 다시 읽기(`reloadKey`)는 커밋 툴 윈도우의
//   미리보기(M2-16 — git/commit/staging-changes.tsx)가 쓴다.

import type { DiffFile, DiffSide, GitDiff } from "@pitwall/rpc-client";
import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, ChevronUp } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";

import { useEditorFont } from "../../editor/editor-font";
import { type ProjectStrings, useProjectStrings } from "../../layout/strings";
import {
  type DiffContent,
  type DiffController,
  type DiffMode,
  DiffView,
  type DiffViewSlots,
} from "../diff/diff-view";
import { gitDocumentSource, loadFileTexts, sidesOf } from "./source";

/** 창 안 diff 뷰가 함께 쓰는 보기(나란히 · 통합). 기억하지 않는다 — 창을 새로 열면 나란히. */
export const diffPreferences = create<{ mode: DiffMode }>(() => ({ mode: "split" }));

type DocStrings = ProjectStrings["git"]["documents"];

const STATUS_LETTER: Record<DiffFile["status"], { letter: string; tone: string }> = {
  added: { letter: "A", tone: "text-git-added" },
  modified: { letter: "M", tone: "text-git-modified" },
  deleted: { letter: "D", tone: "text-git-deleted" },
  renamed: { letter: "R", tone: "text-git-renamed" },
  typeChanged: { letter: "T", tone: "text-git-modified" },
};

interface Row {
  key: string;
  depth: number;
  name: string;
  /** 폴더 행이면 없다. */
  file?: DiffFile;
  /** 폴더 행의 경로(펼침 키). */
  dir?: string;
}

interface Dir {
  dirs: Map<string, Dir>;
  files: DiffFile[];
}

/** 경로들의 트리 행(폴더 먼저, 이름순). 접힌 폴더의 안은 빼고, 하나뿐인 하위 폴더는 이어 붙인다. */
export function treeRows(files: readonly DiffFile[], collapsed: ReadonlySet<string>): Row[] {
  const root: Dir = { dirs: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    let dir = root;
    for (const part of parts.slice(0, -1)) {
      let next = dir.dirs.get(part);
      if (!next) {
        next = { dirs: new Map(), files: [] };
        dir.dirs.set(part, next);
      }
      dir = next;
    }
    dir.files.push(file);
  }
  const rows: Row[] = [];
  const walk = (dir: Dir, prefix: string, depth: number) => {
    for (const name of [...dir.dirs.keys()].sort()) {
      let child = dir.dirs.get(name)!;
      let label = name;
      while (child.files.length === 0 && child.dirs.size === 1) {
        const [only, next] = [...child.dirs][0]!;
        label = `${label}/${only}`;
        child = next;
      }
      const path = prefix ? `${prefix}/${label}` : label;
      rows.push({ key: `d:${path}`, depth, name: label, dir: path });
      if (!collapsed.has(path)) walk(child, path, depth + 1);
    }
    for (const file of [...dir.files].sort((a, b) => a.path.localeCompare(b.path))) {
      rows.push({ key: `f:${file.path}`, depth, name: file.path.split("/").pop()!, file });
    }
  };
  walk(root, "", 0);
  return rows;
}

const ROW = 22;

function FileTree({
  files,
  selected,
  onSelect,
}: {
  files: readonly DiffFile[];
  selected: string | undefined;
  onSelect(path: string): void;
}) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const rows = useMemo(() => treeRows(files, collapsed), [files, collapsed]);
  const scroller = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 10,
    // 시험 환경(크기를 재지 못함)에서도 앞 행들이 그려지게 처음 높이를 준다.
    initialRect: { width: 280, height: 600 },
  });
  const fileRows = rows.filter((r) => r.file);
  const move = (dir: 1 | -1) => {
    const i = fileRows.findIndex((r) => r.file!.path === selected);
    const next = fileRows[Math.min(fileRows.length - 1, Math.max(0, i + dir))];
    if (next) {
      onSelect(next.file!.path);
      virtual.scrollToIndex(rows.indexOf(next));
    }
  };
  return (
    <div
      ref={scroller}
      role="tree"
      tabIndex={0}
      data-git-changes-tree=""
      onKeyDown={(e) => {
        if (e.key === "ArrowDown") move(1);
        else if (e.key === "ArrowUp") move(-1);
        else return;
        e.preventDefault();
      }}
      className="h-full overflow-auto outline-none"
    >
      <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
        {virtual.getVirtualItems().map((item) => {
          const row = rows[item.index]!;
          const style = {
            position: "absolute" as const,
            top: 0,
            left: 0,
            right: 0,
            height: ROW,
            transform: `translateY(${item.start}px)`,
            paddingLeft: 6 + row.depth * 14,
          };
          if (row.dir !== undefined) {
            const open = !collapsed.has(row.dir);
            return (
              <div
                key={row.key}
                role="treeitem"
                aria-expanded={open}
                style={style}
                onClick={() =>
                  setCollapsed((s) => {
                    const next = new Set(s);
                    if (open) next.add(row.dir!);
                    else next.delete(row.dir!);
                    return next;
                  })
                }
                className="flex cursor-default items-center gap-1 truncate text-ui-muted-foreground hover:bg-ui-hover"
              >
                {open ? (
                  <ChevronDown size={12} aria-hidden />
                ) : (
                  <ChevronRight size={12} aria-hidden />
                )}
                {row.name}
              </div>
            );
          }
          const file = row.file!;
          const mark = STATUS_LETTER[file.status];
          const active = file.path === selected;
          return (
            <div
              key={row.key}
              role="treeitem"
              aria-selected={active}
              data-git-change-path={file.path}
              title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
              style={style}
              onClick={() => onSelect(file.path)}
              className={`flex cursor-default items-center gap-1.5 truncate pr-2 ${active ? "bg-ui-active" : "hover:bg-ui-hover"} ${file.generated ? "text-ui-muted-foreground" : ""}`}
            >
              <span className="w-3 flex-none" />
              <span className={`flex-none font-mono text-[11px] ${mark.tone}`}>{mark.letter}</span>
              <span className="truncate">{row.name}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Notice({ text, children }: { text: string; children?: ReactNode }) {
  return (
    <div
      role="status"
      className="flex h-full flex-col items-center justify-center gap-2 bg-editor-background px-4 text-center text-ui-muted-foreground"
    >
      <p>{text}</p>
      {children}
    </div>
  );
}

const short = (hash: string | null | undefined) => (hash ? hash.slice(0, 7) : "");

/** 파일 머리의 부가 설명(이름 바뀜 · 모드). */
function fileFacts(file: DiffFile, T: DocStrings["diff"]): string[] {
  const facts: string[] = [];
  if (file.oldPath) facts.push(T.renamed(file.oldPath));
  if (file.oldMode && file.newMode && file.oldMode !== file.newMode)
    facts.push(T.modeChange(file.oldMode, file.newMode));
  return facts;
}

type Loaded =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "unreadable" }
  | { kind: "ready"; content: DiffContent };

/** 고른 파일 하나의 diff(접기 · 안내 · 내용 읽기). */
function FileDiffPane({
  diff,
  file,
  from,
  to,
  worktree,
  ignoreWhitespace,
  expanded,
  onExpand,
  onController,
  slots,
}: {
  diff: GitDiff;
  file: DiffFile;
  from: DiffSide | undefined;
  to: DiffSide;
  worktree: string | undefined;
  ignoreWhitespace: boolean;
  expanded: boolean;
  onExpand(): void;
  onController(c: DiffController | undefined): void;
  slots?: DiffViewSlots;
}) {
  const T = useProjectStrings().git.documents;
  const source = gitDocumentSource((s) => s.source);
  const mode = diffPreferences((s) => s.mode);
  const font = useEditorFont();
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const collapsedGenerated = file.generated && !expanded;
  const skip =
    collapsedGenerated ||
    file.binary ||
    file.tooLarge ||
    file.submodule !== null ||
    file.error !== null;

  useEffect(() => {
    if (skip || !source) return;
    let current = true;
    setLoaded({ kind: "loading" });
    void (async () => {
      let target = file;
      let base = diff;
      if (file.hunksOmitted) {
        // 응답 상한에 걸려 빠진 hunk — 그 파일만 다시 묻는다(M2-04).
        base = await source.diff(
          {
            from,
            to,
            paths: [file.path, ...(file.oldPath ? [file.oldPath] : [])],
            ignoreWhitespace,
          },
          worktree,
        );
        target = base.files.find((f) => f.path === file.path) ?? file;
      }
      const texts = await loadFileTexts(source, base, target, sidesOf(base, from, to), worktree);
      if (!current) return;
      if (texts.old.kind !== "text" || texts.new.kind !== "text") {
        setLoaded({ kind: "unreadable" });
        return;
      }
      setLoaded({
        kind: "ready",
        content: {
          file: target,
          oldText: texts.old.text,
          newText: texts.new.text,
          language: texts.old.highlight && texts.new.highlight ? texts.language : null,
        },
      });
    })().catch((e: unknown) => {
      if (current)
        setLoaded({ kind: "failed", message: e instanceof Error ? e.message : String(e) });
    });
    return () => {
      current = false;
    };
  }, [diff, file, from, to, worktree, ignoreWhitespace, skip, source]);

  const phrases = useMemo(() => ({ "$ unchanged lines": T.diff.unchangedLines }), [T]);

  let body: ReactNode;
  if (file.error !== null) body = <Notice text={T.diff.error(file.error)} />;
  else if (file.submodule)
    body = (
      <Notice
        text={T.diff.submodule(
          short(file.submodule.old) || T.diff.submoduleNone,
          // 작업 트리 쪽 서브모듈 안에 커밋하지 않은 변경이 있으면 git 처럼 `-dirty` 를 붙인다(M2-16).
          (short(file.submodule.new) || T.diff.submoduleNone) +
            (file.submodule.dirty ? "-dirty" : ""),
        )}
      />
    );
  else if (file.binary) body = <Notice text={T.diff.binary} />;
  else if (file.tooLarge) body = <Notice text={T.diff.tooLarge} />;
  else if (collapsedGenerated)
    body = (
      <Notice text={T.diff.generated(file.added, file.removed)}>
        <button
          type="button"
          data-git-expand=""
          onClick={onExpand}
          className="rounded-sm border border-ui-border px-2 py-0.5 text-ui-foreground hover:bg-ui-hover"
        >
          {T.diff.expand}
        </button>
      </Notice>
    );
  else if (loaded.kind === "loading") body = <Notice text={T.loading} />;
  else if (loaded.kind === "failed") body = <Notice text={T.failed(loaded.message)} />;
  else if (loaded.kind === "unreadable") body = <Notice text={T.diff.sideUnreadable} />;
  else if (loaded.content.file.hunks.length === 0 && !file.hunksOmitted)
    body = <Notice text={T.diff.noContentChange} />;
  else
    body = (
      <DiffView
        content={loaded.content}
        mode={mode}
        syntax={source?.syntax}
        phrases={phrases}
        font={font}
        slots={slots}
        onController={onController}
      />
    );

  const facts = fileFacts(file, T.diff);
  return (
    <div className="flex h-full min-h-0 flex-col" data-git-file-diff={file.path}>
      {facts.length > 0 && (
        <div className="flex-none truncate border-b border-ui-border px-2 py-0.5 text-[12px] text-ui-muted-foreground">
          {facts.join(" · ")}
        </div>
      )}
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}

function DiffToolbar({
  ignoreWhitespace,
  onIgnoreWhitespace,
  controller,
  path,
}: {
  ignoreWhitespace: boolean;
  onIgnoreWhitespace(value: boolean): void;
  controller: DiffController | undefined;
  path: string | undefined;
}) {
  const T = useProjectStrings().git.documents.diff;
  const mode = diffPreferences((s) => s.mode);
  const button =
    "flex h-[18px] items-center rounded-sm px-1 text-ui-muted-foreground hover:bg-ui-hover disabled:opacity-40";
  return (
    <div className="flex flex-none items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px]">
      <span className="min-w-0 flex-1 truncate" title={path}>
        {path}
      </span>
      {controller && (
        <span data-git-diff-count="" className="text-ui-muted-foreground">
          {T.changes(controller.model.groups.length)}
        </span>
      )}
      <button
        type="button"
        title={T.previous}
        aria-label={T.previous}
        disabled={!controller}
        onClick={() => controller?.go(-1)}
        className={button}
      >
        <ChevronUp size={12} aria-hidden />
      </button>
      <button
        type="button"
        title={T.next}
        aria-label={T.next}
        disabled={!controller}
        onClick={() => controller?.go(1)}
        className={button}
      >
        <ChevronDown size={12} aria-hidden />
      </button>
      <label className="flex items-center gap-1">
        <input
          type="checkbox"
          data-git-ignore-whitespace=""
          checked={ignoreWhitespace}
          onChange={(e) => onIgnoreWhitespace(e.target.checked)}
        />
        {T.ignoreWhitespace}
      </label>
      <ToggleGroup.Root
        type="single"
        aria-label={T.mode}
        value={mode}
        onValueChange={(value) => {
          if (value) diffPreferences.setState({ mode: value as DiffMode });
        }}
        className="flex items-center"
      >
        {(["split", "unified"] as const).map((m) => (
          <ToggleGroup.Item
            key={m}
            value={m}
            data-git-diff-mode={m}
            className="rounded-sm px-1.5 text-ui-muted-foreground hover:bg-ui-hover data-[state=on]:bg-ui-active data-[state=on]:text-ui-foreground"
          >
            {m === "split" ? T.split : T.unified}
          </ToggleGroup.Item>
        ))}
      </ToggleGroup.Root>
    </div>
  );
}

/**
 * `git.diff { from, to }` 의 바뀐 파일과 diff. `header` 는 위에 두는 것(커밋 메시지 · 비교 머리). 파일을 다 읽은 뒤 첫 파일을 고른다.
 */
export function ChangesView({
  from,
  to,
  worktree,
  header,
  paths,
  tree = true,
  slots,
  reloadKey,
}: {
  from: DiffSide | undefined;
  to: DiffSide;
  worktree: string | undefined;
  header?: ReactNode;
  /** 이 경로들만(`git.diff` 의 `paths`). */
  paths?: readonly string[];
  /** 파일 트리를 둔다. 파일 하나의 diff 문서는 두지 않는다. */
  tree?: boolean;
  /** diff 뷰의 확장 지점. 공백 무시 여부를 받는다 — 같은 값이면 같은 객체를 돌려준다. */
  slots?: (ignoreWhitespace: boolean) => DiffViewSlots | undefined;
  /** 바뀌면 `git.diff` 를 다시 읽는다(스테이징한 뒤 등). */
  reloadKey?: unknown;
}) {
  const T = useProjectStrings().git.documents;
  const source = gitDocumentSource((s) => s.source);
  const [ignoreWhitespace, setIgnoreWhitespace] = useState(false);
  const [state, setState] = useState<
    { kind: "loading" } | { kind: "failed"; message: string } | { kind: "ready"; diff: GitDiff }
  >({ kind: "loading" });
  const [selected, setSelected] = useState<string>();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [controller, setController] = useState<DiffController>();

  useEffect(() => {
    if (!source) return;
    let current = true;
    source.diff({ from, to, ignoreWhitespace, ...(paths && { paths: [...paths] }) }, worktree).then(
      (diff) => {
        if (!current) return;
        setState({ kind: "ready", diff });
        setSelected((s) =>
          s !== undefined && diff.files.some((f) => f.path === s) ? s : diff.files[0]?.path,
        );
      },
      (e: unknown) => {
        if (current)
          setState({ kind: "failed", message: e instanceof Error ? e.message : String(e) });
      },
    );
    return () => {
      current = false;
    };
    // `from` · `to` 는 문서마다 바뀌지 않는 값이다 — 모양으로 견준다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    source,
    JSON.stringify(from),
    JSON.stringify(to),
    JSON.stringify(paths),
    worktree,
    ignoreWhitespace,
    reloadKey,
  ]);

  const diff = state.kind === "ready" ? state.diff : undefined;
  const file = diff?.files.find((f) => f.path === selected);
  return (
    <div className="flex h-full min-h-0 flex-col bg-editor-background">
      {header}
      <div className="flex min-h-0 flex-1">
        {tree && (
          <div className="flex w-[280px] flex-none flex-col border-r border-ui-border">
            <div className="flex-none border-b border-ui-border px-2 py-0.5 text-[12px] text-ui-muted-foreground">
              {diff ? (diff.files.length > 0 ? T.files(diff.files.length) : T.noFiles) : T.loading}
            </div>
            <div className="min-h-0 flex-1">
              {diff && <FileTree files={diff.files} selected={selected} onSelect={setSelected} />}
            </div>
          </div>
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          <DiffToolbar
            ignoreWhitespace={ignoreWhitespace}
            onIgnoreWhitespace={setIgnoreWhitespace}
            controller={controller}
            path={file?.path}
          />
          <div className="min-h-0 flex-1">
            {state.kind === "failed" ? (
              <Notice text={T.failed(state.message)} />
            ) : !diff ? (
              <Notice text={T.loading} />
            ) : !file ? (
              <Notice text={diff.files.length > 0 ? T.pickFile : T.noFiles} />
            ) : (
              <FileDiffPane
                key={file.path}
                diff={diff}
                file={file}
                from={from}
                to={to}
                worktree={worktree}
                ignoreWhitespace={ignoreWhitespace}
                expanded={expanded.has(file.path)}
                onExpand={() => setExpanded((s) => new Set(s).add(file.path))}
                onController={setController}
                slots={slots?.(ignoreWhitespace)}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
