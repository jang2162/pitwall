// git 문서 탭의 내용(src/editor/editor-panel.tsx 가 params `git` 이 있으면 이것을 그린다). 종류(spec.ts)마다:
//
// - 커밋 상세(F5-3): 위에 메시지 · 작성자 · 커미터 · 부모(누르면 그 커밋 상세) · ref, 아래에 바뀐 파일 트리 + diff(첫 부모와 —
//   머지 커밋도 첫 부모, 루트 커밋은 빈 트리와. `git.diff` 의 규칙 그대로).
// - 두 커밋 비교(F5-4): `a` → `b` 의 바뀐 파일과 diff.
// - 리비전의 파일(F5-19): 그 커밋의 파일 내용을 읽기 전용 에디터로. 위 막대(10-ui.md 「코드 에디터」 의 미니 툴바)에 리비전과
//   「읽기 전용」.
// - diff: 파일 하나의 diff(트리 없이). 작업 트리 · 스테이징된 diff 는 커밋 툴 윈도우의 미리보기라 hunk · 줄 체크를 단다(M2-16 —
//   git/commit/staging-changes.tsx).
// - 머지: 충돌 파일 하나의 3-way 머지(src/git/merge/merge-document.tsx, M2-19).
// - 파일 히스토리: 그 파일의 커밋 목록 + 고른 커밋에서의 diff(src/git/history/history-document.tsx, M2-18).

import { EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  lineNumbers,
} from "@codemirror/view";
import type { GitShow, Signature } from "@pitwall/rpc-client";
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useRef, useState } from "react";

import { useEditorFont } from "../../editor/editor-font";
import type { EditorTabParams } from "../../editor/editor-tabs";
import { fontTheme } from "../../editor/extensions/operations";
import { editorTheme } from "../../editor/extensions/theme";
import { useI18n } from "../../i18n/i18n-provider";
import { useKeymap } from "../../keymap/keymap-provider";
import { useProjectStrings } from "../../layout/strings";
import { normalizeText } from "../diff/diff-model";
import { revisionHighlight, revisionSyntax } from "../diff/revision-syntax";
import { HistoryDocument } from "../history/history-document";
import { MergeDocument } from "../merge/merge-document";
import { isStagingDiff, StagingChanges } from "../commit/staging-changes";
import { ChangesView } from "./changes-view";
import { gitDocumentSource, revisionText, type SideText } from "./source";
import type { GitDocumentSpec } from "./spec";

const short = (hash: string) => hash.slice(0, 7);

function Status({ text }: { text: string }) {
  return (
    <div
      role="status"
      className="flex h-full items-center justify-center bg-editor-background px-4 text-center text-ui-muted-foreground"
    >
      {text}
    </div>
  );
}

function when(sig: Signature, locale: string | undefined): string {
  return new Date(sig.time * 1000).toLocaleString(locale);
}

function CommitHeader({ show }: { show: GitShow }) {
  const T = useProjectStrings().git.documents;
  const { registry } = useKeymap();
  const { locales } = useI18n();
  const locale = locales[0];
  const [subject, ...rest] = show.message.split("\n");
  const body = rest.join("\n").trim();
  const sameCommitter =
    show.committer.name === show.author.name && show.committer.email === show.author.email;
  return (
    <div
      data-git-commit-header={show.commit}
      className="flex max-h-[40%] flex-none flex-col gap-1 overflow-auto border-b border-ui-border px-3 py-2 text-[12px]"
    >
      <div className="text-[13px] font-semibold text-ui-foreground">{subject}</div>
      {body && <pre className="font-sans whitespace-pre-wrap text-ui-foreground">{body}</pre>}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-ui-muted-foreground">
        <span className="font-mono select-text">{show.commit}</span>
        <span>
          {T.author}: {show.author.name} &lt;{show.author.email}&gt; · {when(show.author, locale)}
        </span>
        {!sameCommitter && (
          <span>
            {T.committer}: {show.committer.name} &lt;{show.committer.email}&gt; ·{" "}
            {when(show.committer, locale)}
          </span>
        )}
        <span className="flex items-center gap-1">
          {T.parents}:
          {show.parents.length === 0 ? (
            <span>{show.shallow ? T.shallow : T.noParents}</span>
          ) : (
            show.parents.map((p) => (
              <button
                key={p}
                type="button"
                data-git-parent={p}
                onClick={() => registry.run("git.showCommit", { rev: p })}
                className="font-mono text-ui-accent hover:underline"
              >
                {short(p)}
              </button>
            ))
          )}
        </span>
        {show.refs.map((r) => (
          <span
            key={r.fullName}
            title={r.fullName}
            className="rounded-sm border border-ui-border px-1 text-ui-foreground"
          >
            {r.name}
          </span>
        ))}
      </div>
    </div>
  );
}

function CommitDocument({ rev, worktree }: { rev: string; worktree: string | undefined }) {
  const T = useProjectStrings().git.documents;
  const source = gitDocumentSource((s) => s.source);
  const [show, setShow] = useState<GitShow | { failed: string }>();
  useEffect(() => {
    if (!source) return;
    let current = true;
    source.show(rev, worktree).then(
      (s) => current && setShow(s),
      (e: unknown) => current && setShow({ failed: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      current = false;
    };
  }, [source, rev, worktree]);
  if (show && "failed" in show) return <Status text={T.failed(show.failed)} />;
  return (
    <ChangesView
      to={{ kind: "commit", rev }}
      from={undefined}
      worktree={worktree}
      header={show ? <CommitHeader show={show} /> : undefined}
    />
  );
}

function RevisionDocument({
  rev,
  path,
  worktree,
}: {
  rev: string;
  path: string;
  worktree: string | undefined;
}) {
  const T = useProjectStrings();
  const D = T.git.documents;
  const source = gitDocumentSource((s) => s.source);
  const font = useEditorFont();
  const host = useRef<HTMLDivElement>(null);
  const [text, setText] = useState<
    { side: SideText; language: string | null } | { failed: string } | undefined
  >();
  useEffect(() => {
    if (!source) return;
    let current = true;
    void (async () => {
      const side = await revisionText(source, rev, path, worktree);
      let language: string | null = null;
      if (side.kind === "text" && side.highlight) {
        const top = await source.workTree(worktree);
        language = await source
          .detect(`${top}/${path}`, side.text.slice(0, 50 * 1024), worktree)
          .catch(() => null);
      }
      if (current) setText({ side, language });
    })().catch((e: unknown) => {
      if (current) setText({ failed: e instanceof Error ? e.message : String(e) });
    });
    return () => {
      current = false;
    };
  }, [source, rev, path, worktree]);

  const ready = text && !("failed" in text) && text.side.kind === "text" ? text : undefined;
  useEffect(() => {
    const parent = host.current;
    if (!parent || !ready || ready.side.kind !== "text") return;
    const content = normalizeText(ready.side.text);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: content,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          highlightSpecialChars(),
          drawSelection(),
          editorTheme,
          fontTheme(font),
          EditorState.readOnly.of(true),
          EditorState.lineSeparator.of("\n"),
          revisionHighlight(
            source?.syntax && revisionSyntax(source.syntax, ready.language, content),
          ),
        ],
      }),
    });
    return () => view.destroy();
  }, [ready, source, font]);

  let body;
  if (!text) body = <Status text={D.loading} />;
  else if ("failed" in text) body = <Status text={D.failed(text.failed)} />;
  else if (text.side.kind === "binary") body = <Status text={D.diff.binary} />;
  else if (text.side.kind === "tooLarge") body = <Status text={D.diff.tooLarge} />;
  else if (text.side.kind === "missing") body = <Status text={D.diff.sideUnreadable} />;
  else body = <div ref={host} data-git-revision-text="" className="h-full min-h-0" />;
  return (
    <div className="flex h-full min-h-0 flex-col bg-editor-background">
      <div
        data-git-revision-bar={rev}
        className="flex flex-none items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px] text-ui-muted-foreground"
      >
        <span className="font-mono">{D.revisionBar(short(rev))}</span>
        <span className="min-w-0 truncate" title={path}>
          {path}
        </span>
      </div>
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}

export function GitDocumentPanel({
  params,
  spec,
  panelId,
}: {
  params: EditorTabParams;
  spec: GitDocumentSpec;
  /** 탭(dockview 패널) id. */
  panelId: string;
}) {
  const T = useProjectStrings().git.documents;
  const source = gitDocumentSource((s) => s.source);
  if (!source) return <Status text={T.loading} />;
  switch (spec.kind) {
    case "commit":
      return <CommitDocument rev={spec.rev} worktree={params.worktree} />;
    case "compare":
      return (
        <ChangesView
          from={{ kind: "commit", rev: spec.a }}
          to={{ kind: "commit", rev: spec.b }}
          worktree={params.worktree}
          header={
            <div className="flex-none border-b border-ui-border px-3 py-1.5 font-mono text-[12px]">
              {T.compareHeading(short(spec.a), short(spec.b))}
            </div>
          }
        />
      );
    case "revision":
      return <RevisionDocument rev={spec.rev} path={spec.path} worktree={params.worktree} />;
    case "diff":
      // 작업 트리 · 스테이징된 diff 는 hunk · 줄 체크(부분 스테이징)를 단다(M2-16).
      if (isStagingDiff(spec)) return <StagingChanges spec={spec} worktree={params.worktree} />;
      return (
        <ChangesView
          from={spec.from}
          to={spec.to}
          worktree={params.worktree}
          paths={[spec.path, ...(spec.oldPath ? [spec.oldPath] : [])]}
          tree={false}
        />
      );
    case "merge":
      return <MergeDocument path={spec.path} worktree={params.worktree} panelId={panelId} />;
    case "history":
      return <HistoryDocument path={spec.path} worktree={params.worktree} />;
  }
}

export type GitDocumentPanelProps = IDockviewPanelProps<EditorTabParams>;
