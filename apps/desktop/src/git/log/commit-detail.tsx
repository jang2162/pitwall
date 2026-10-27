// Git 로그의 오른쪽 커밋 상세(docs/design/13-m2-plan.md M2-14): 커밋 하나면 변경 파일 트리(`git.diff` — 커밋 ↔ 첫 부모) · 메시지 ·
// 작성자 · 커미터 · 부모(누르면 그 커밋을 고른다) · 포함한 브랜치 · 태그(`git.containedIn` — 비싸서 상세를 받은 뒤 따로), 커밋 둘이면
// 두 커밋 비교(F5-4 — 옛 쪽 → 새 쪽의 바뀐 파일)와 비교 탭 열기. 파일을 더블클릭하면 그 리비전의 파일을 읽기 전용으로 연다
// (M2-15 의 `git.openRevisionFile`). 생성 파일(lockfile 등)은 흐리게 「생성」 을 단다(F5-3).
//
// 고른 커밋이 바뀌면 앞 요청의 답은 버린다.

import type { DiffFile, GitContainedIn, GitShow, RpcClient } from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";

import { short } from "./log-ops";
import type { GitLogStrings } from "./strings";

export interface DetailSource {
  client: RpcClient;
  projectId: string;
  worktree?: string;
}

type Load<T> =
  | { kind: "loading" }
  | { kind: "done"; value: T }
  | { kind: "error"; message: string };

function message(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}

/** `run` 을 `key` 가 바뀔 때마다 부르고 그 답을 들고 있는다. */
function useLoad<T>(key: string | undefined, run: () => Promise<T>): Load<T> | undefined {
  const [state, setState] = useState<{ key: string; load: Load<T> }>();
  // `run` 은 `key` 로 정해진다 — 렌더마다 새 함수라도 `key` 가 같으면 다시 부르지 않는다.
  const runRef = useRef(run);
  runRef.current = run;
  useEffect(() => {
    if (key === undefined) return;
    let current = true;
    setState({ key, load: { kind: "loading" } });
    runRef.current().then(
      (value) => current && setState({ key, load: { kind: "done", value } }),
      (e: unknown) => current && setState({ key, load: { kind: "error", message: message(e) } }),
    );
    return () => {
      current = false;
    };
  }, [key]);
  if (key === undefined) return undefined;
  return state && state.key === key ? state.load : { kind: "loading" };
}

/** 파일 트리의 행: 폴더(깊이)와 파일. 폴더는 한 갈래뿐이면 이어 붙인다(`src/main`). */
export type FileRow =
  | { kind: "dir"; path: string; depth: number }
  | { kind: "file"; file: DiffFile; name: string; depth: number };

export function fileRows(files: readonly DiffFile[]): FileRow[] {
  interface Dir {
    dirs: Map<string, Dir>;
    files: DiffFile[];
  }
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
  const rows: FileRow[] = [];
  const walk = (dir: Dir, depth: number) => {
    for (const [name, sub] of [...dir.dirs].sort(([a], [b]) => a.localeCompare(b))) {
      let path = name;
      let at = sub;
      while (at.files.length === 0 && at.dirs.size === 1) {
        const [childName, child] = [...at.dirs][0]!;
        path = `${path}/${childName}`;
        at = child;
      }
      rows.push({ kind: "dir", path, depth });
      walk(at, depth + 1);
    }
    for (const file of dir.files)
      rows.push({ kind: "file", file, name: file.path.split("/").pop()!, depth });
  };
  walk(root, 0);
  return rows;
}

const LETTER: Record<DiffFile["status"], string> = {
  added: "A",
  deleted: "D",
  modified: "M",
  renamed: "R",
  typeChanged: "T",
};

const TONE: Record<DiffFile["status"], string> = {
  added: "text-git-added",
  deleted: "text-git-deleted",
  modified: "text-git-modified",
  renamed: "text-git-renamed",
  typeChanged: "text-git-modified",
};

function FileTree({
  files,
  rev,
  strings,
  onOpenFile,
}: {
  files: readonly DiffFile[];
  rev: string;
  strings: GitLogStrings["detail"];
  onOpenFile?(rev: string, path: string): void;
}) {
  return (
    <ul role="tree" aria-label={strings.files(files.length)} className="text-[12px]">
      {fileRows(files).map((row) =>
        row.kind === "dir" ? (
          <li
            key={`d:${row.path}`}
            role="treeitem"
            aria-expanded
            className="truncate text-ui-muted-foreground"
            style={{ paddingLeft: 4 + row.depth * 12 }}
          >
            {row.path}
          </li>
        ) : (
          <li
            key={`f:${row.file.path}`}
            role="treeitem"
            title={row.file.oldPath ? `${row.file.oldPath} → ${row.file.path}` : row.file.path}
            className={`flex cursor-default items-center gap-1 truncate hover:bg-ui-hover ${row.file.generated ? "opacity-60" : ""}`}
            style={{ paddingLeft: 4 + row.depth * 12 }}
            onDoubleClick={() => {
              // 지운 파일은 새 리비전에 없다 — 부모 쪽을 연다.
              onOpenFile?.(row.file.status === "deleted" ? `${rev}^` : rev, row.file.path);
            }}
          >
            <span className="truncate">{row.name}</span>
            <span className={`ml-auto shrink-0 font-mono ${TONE[row.file.status]}`}>
              {LETTER[row.file.status]}
            </span>
            {row.file.generated && (
              <span className="shrink-0 text-[10px] text-ui-muted-foreground">
                {strings.generated}
              </span>
            )}
          </li>
        ),
      )}
    </ul>
  );
}

export interface CommitDetailProps {
  source: DetailSource | undefined;
  /** 고른 커밋(로그 차례 — 새것부터). */
  selected: readonly string[];
  strings: GitLogStrings["detail"];
  formatDate(seconds: number): string;
  onSelectCommit(hash: string): void;
  /** 리비전의 파일 · 커밋 상세 탭 · 비교 탭(M2-15 의 액션). 그 액션이 이 창에 없으면 없다 — 버튼을 두지 않는다. */
  onOpenFile?(rev: string, path: string): void;
  onOpenCommit?(rev: string): void;
  onCompare?(a: string, b: string): void;
}

export function CommitDetail(props: CommitDetailProps) {
  const { selected, strings } = props;
  if (selected.length === 0)
    return <p className="p-2 text-[12px] text-ui-muted-foreground">{strings.none}</p>;
  if (selected.length > 2)
    return (
      <p className="p-2 text-[12px] text-ui-muted-foreground">{strings.many(selected.length)}</p>
    );
  if (selected.length === 2) {
    // 로그 차례가 새것부터이므로 뒤가 옛 쪽이다.
    const [b, a] = selected as [string, string];
    return <CompareDetail {...props} a={a} b={b} />;
  }
  return <SingleDetail {...props} rev={selected[0]!} />;
}

function SingleDetail(props: CommitDetailProps & { rev: string }) {
  const { source, rev, strings } = props;
  const base = source
    ? { projectId: source.projectId, worktree: source.worktree ?? null }
    : undefined;
  const key = source ? rev : undefined;
  const show = useLoad<GitShow>(key, () => source!.client.call("git.show", { ...base!, rev }));
  const diff = useLoad<DiffFile[]>(key, () =>
    source!.client.call("git.diff", { ...base!, to: { kind: "commit", rev } }).then((d) => d.files),
  );
  const contained = useLoad<GitContainedIn>(show?.kind === "done" ? rev : undefined, () =>
    source!.client.call("git.containedIn", { ...base!, rev }),
  );
  if (show?.kind === "error")
    return <p className="p-2 text-[12px] text-ui-accent">{strings.failed(show.message)}</p>;
  const commit = show?.kind === "done" ? show.value : undefined;
  return (
    <div data-testid="git-log-detail" className="flex flex-col gap-2 p-2 text-[12px]">
      <section>
        {diff?.kind === "done" ? (
          <>
            <h3 className="font-semibold">{strings.files(diff.value.length)}</h3>
            <FileTree
              files={diff.value}
              rev={rev}
              strings={strings}
              onOpenFile={props.onOpenFile}
            />
          </>
        ) : diff?.kind === "error" ? (
          <p className="text-ui-accent">{strings.failed(diff.message)}</p>
        ) : (
          <p className="text-ui-muted-foreground">{strings.filesLoading}</p>
        )}
      </section>
      {commit && (
        <>
          <hr className="border-ui-border" />
          <pre className="font-sans break-words whitespace-pre-wrap select-text">
            {commit.message.trimEnd()}
          </pre>
          <dl className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
            <dt className="text-ui-muted-foreground">{strings.author}</dt>
            <dd className="select-text">
              {commit.author.name} &lt;{commit.author.email}&gt; ·{" "}
              {props.formatDate(commit.author.time)}
            </dd>
            <dt className="text-ui-muted-foreground">{strings.committer}</dt>
            <dd className="select-text">
              {commit.committer.name} · {props.formatDate(commit.committer.time)}
            </dd>
            <dt className="text-ui-muted-foreground">{strings.parents}</dt>
            <dd className="flex flex-wrap gap-1">
              {commit.parents.length === 0
                ? strings.root
                : commit.parents.map((p) => (
                    <button
                      key={p}
                      type="button"
                      className="font-mono text-ui-accent hover:underline"
                      onClick={() => props.onSelectCommit(p)}
                    >
                      {short(p)}
                    </button>
                  ))}
            </dd>
            <dt className="text-ui-muted-foreground">{strings.containedIn}</dt>
            <dd data-testid="git-log-contained" className="flex flex-wrap gap-1">
              {contained?.kind === "done"
                ? contained.value.refs.length === 0
                  ? strings.containedNone
                  : contained.value.refs.map((r) => (
                      <span key={r.fullName} className="rounded-sm border border-ui-border px-1">
                        {r.name}
                      </span>
                    ))
                : contained?.kind === "error"
                  ? strings.failed(contained.message)
                  : strings.containedLoading}
            </dd>
          </dl>
          {props.onOpenCommit && (
            <button
              type="button"
              className="self-start rounded-sm border border-ui-border px-2 py-0.5 hover:bg-ui-hover"
              onClick={() => props.onOpenCommit?.(rev)}
            >
              {strings.openCommit}
            </button>
          )}
        </>
      )}
    </div>
  );
}

function CompareDetail(props: CommitDetailProps & { a: string; b: string }) {
  const { source, a, b, strings } = props;
  const key = source ? `${a}..${b}` : undefined;
  const diff = useLoad<DiffFile[]>(key, () =>
    source!.client
      .call("git.diff", {
        projectId: source!.projectId,
        worktree: source!.worktree ?? null,
        from: { kind: "commit", rev: a },
        to: { kind: "commit", rev: b },
      })
      .then((d) => d.files),
  );
  return (
    <div data-testid="git-log-compare" className="flex flex-col gap-2 p-2 text-[12px]">
      <h3 className="font-semibold">{strings.compare(short(a), short(b))}</h3>
      {props.onCompare && (
        <button
          type="button"
          className="self-start rounded-sm border border-ui-border px-2 py-0.5 hover:bg-ui-hover"
          onClick={() => props.onCompare?.(a, b)}
        >
          {strings.openCompare}
        </button>
      )}
      {diff?.kind === "done" ? (
        <>
          <h3 className="font-semibold">{strings.files(diff.value.length)}</h3>
          <FileTree files={diff.value} rev={b} strings={strings} onOpenFile={props.onOpenFile} />
        </>
      ) : diff?.kind === "error" ? (
        <p className="text-ui-accent">{strings.failed(diff.message)}</p>
      ) : (
        <p className="text-ui-muted-foreground">{strings.filesLoading}</p>
      )}
    </div>
  );
}
