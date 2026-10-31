// DDL 문서(14-m3-plan.md M3-13 — 탐색기 우클릭 「DDL 보기」 · `⌘B`): 에디터 탭 안의 읽기 전용 SQL. 탭의 꼴은 DB 문서 틀
// (src/db/documents/spec.ts 의 `ddl`)이다.
//
// - 내용은 `db.schema.ddl`(M3-10 — 메타데이터용 연결 · 캐시)이고 탭에 저장하지 않는다. 이 창에서 사람이 연 탭(`openDdlDocument`)은
//   곧바로 읽고, 다시 시작해 되살린 탭은 [DDL 읽기] 를 누를 때 읽는다 — 마운트만으로 연결을 열지 않는다(결정 M3-D13, 틀의 머리 주석).
// - 같은 객체의 DDL 탭이 이미 열려 있으면 새로 만들지 않고 그 탭을 앞으로 가져온다.
// - 읽기가 비밀 · 확인 필요(3101 · 3102)로 실패하면 접근 묻기(src/db/safety/access-prompt.tsx)로 묻고 넣었으면 다시 읽는다. 탭은
//   연결 이름을 모르므로 다이얼로그 제목에는 연결 id 를 쓴다.
// - 구문 강조는 구문 분석 서비스의 SQL 문법(내장 확장 lang-sql)이다 — 리비전 파일(src/git/documents)과 같은 길.

import { EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  lineNumbers,
} from "@codemirror/view";
import type { ObjectRef } from "@pitwall/rpc-client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useDbAccessPrompt } from "../safety/access-prompt";

import { useEditorFont } from "../../editor/editor-font";
import { openTab, tabParams } from "../../editor/editor-tabs";
import type { EditorWorkspace } from "../../editor/editor-workspace";
import type { EditorFont } from "../../editor/extensions";
import { fontTheme } from "../../editor/extensions/operations";
import { editorTheme } from "../../editor/extensions/theme";
import { normalizeText } from "../../git/diff/diff-model";
import {
  revisionHighlight,
  revisionSyntax,
  type StaticSyntaxSource,
} from "../../git/diff/revision-syntax";
import { useHost } from "../../rpc/host";
import type { DbDocumentProps } from "../documents/db-document-panel";
import { dbDocumentKey, type DbDocumentSpec, newDbTabId } from "../documents/spec";
import { errorText, withDbAccess } from "./explorer-model";
import { useDbExplorerStrings } from "./strings";

/** 구문 분석 서비스의 SQL 언어 이름(Linguist). */
export const SQL_LANGUAGE = "SQL";

type DdlSpec = Extract<DbDocumentSpec, { kind: "ddl" }>;

/** 이 창에서 사람이 연 DDL 탭(곧바로 읽는다). 되살린 탭은 여기 없다. */
const openedHere = new Set<string>();

const sameObject = (a: ObjectRef, b: ObjectRef) =>
  a.name === b.name && a.schema === b.schema && a.database === b.database;

/** 이 창에서 사람이 연 탭인가(곧바로 읽는다). */
export function ddlOpenedHere(tabId: string): boolean {
  return openedHere.has(tabId);
}

/** DDL 탭을 연다(같은 객체의 탭이 있으면 그것을 앞으로). */
export function openDdlDocument(
  workspace: Pick<EditorWorkspace, "api" | "recent">,
  connectionId: string,
  object: ObjectRef,
  title: string,
  worktree: string | undefined,
) {
  const api = workspace.api();
  if (!api) return undefined;
  const existing = api.panels.find((p) => {
    const db = tabParams(p).db;
    return db?.kind === "ddl" && db.connectionId === connectionId && sameObject(db.object, object);
  });
  if (existing) {
    existing.api.setActive();
    return existing;
  }
  const spec: DdlSpec = { kind: "ddl", tabId: newDbTabId(), connectionId, object };
  openedHere.add(spec.tabId);
  const { panel } = openTab(api, {
    path: dbDocumentKey(spec),
    worktree,
    db: spec,
    title,
    recent: workspace.recent(),
  });
  panel.api.setActive();
  return panel;
}

type Loaded =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "text"; text: string }
  | { kind: "failed"; message: string };

export interface DdlViewProps {
  spec: DdlSpec;
  /** DDL 을 읽는다(`db.schema.ddl`). */
  load(): Promise<string>;
  /** 마운트하자마자 읽는가(이 창에서 사람이 연 탭). */
  eager: boolean;
  font?: EditorFont;
  syntax?: StaticSyntaxSource;
}

/** DDL 화면(Host 와 떨어진 몫 — 시험이 그대로 띄운다). */
export function DdlView({ spec, load, eager, font = {}, syntax }: DdlViewProps) {
  const T = useDbExplorerStrings();
  const [loaded, setLoaded] = useState<Loaded>({ kind: eager ? "loading" : "idle" });
  const host = useRef<HTMLDivElement>(null);
  const token = useRef(0);

  const read = useCallback(() => {
    const mine = ++token.current;
    setLoaded({ kind: "loading" });
    load().then(
      (text) => {
        if (token.current === mine) setLoaded({ kind: "text", text: normalizeText(text) });
      },
      (e: unknown) => {
        if (token.current === mine) setLoaded({ kind: "failed", message: errorText(e) });
      },
    );
  }, [load]);

  useEffect(() => {
    if (eager) read();
    return () => {
      token.current++;
    };
    // 탭 하나에 한 번 — 다시 읽기는 사람이 누른다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const text = loaded.kind === "text" ? loaded.text : undefined;
  useEffect(() => {
    const parent = host.current;
    if (!parent || text === undefined) return;
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: text,
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
          revisionHighlight(syntax && revisionSyntax(syntax, SQL_LANGUAGE, text)),
        ],
      }),
    });
    return () => view.destroy();
  }, [text, font, syntax]);

  const { object } = spec;
  const name = [object.database, object.schema, object.name].filter(Boolean).join(".");
  const button =
    "rounded-sm border border-ui-border px-2 py-0.5 text-ui-foreground hover:bg-ui-hover disabled:opacity-40";
  let body;
  if (loaded.kind === "idle")
    body = (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-ui-muted-foreground">
        <p>{T.ddl.restored}</p>
        <button type="button" data-testid="db-ddl-read" className={button} onClick={read}>
          {T.ddl.read}
        </button>
      </div>
    );
  else if (loaded.kind === "loading")
    body = (
      <p role="status" className="p-4 text-ui-muted-foreground">
        {T.loading}
      </p>
    );
  else if (loaded.kind === "failed")
    body = (
      <p role="alert" className="p-4 text-ui-accent">
        {T.failed(loaded.message)}
      </p>
    );
  else body = <div ref={host} data-db-ddl-text="" className="h-full min-h-0" />;
  return (
    <div data-db-ddl={spec.tabId} className="flex h-full min-h-0 flex-col bg-editor-background">
      <div className="flex flex-none items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px] text-ui-muted-foreground">
        <span className="font-mono">{spec.connectionId}</span>
        <span className="min-w-0 flex-1 truncate" title={name}>
          {name}
        </span>
        {loaded.kind !== "idle" && (
          <button
            type="button"
            data-testid="db-ddl-reload"
            className={button}
            disabled={loaded.kind === "loading"}
            onClick={read}
          >
            {T.ddl.reload}
          </button>
        )}
      </div>
      <div className="min-h-0 flex-1">{body}</div>
    </div>
  );
}

/** 에디터 탭 안의 DDL 문서(DB 문서 틀의 `ddl`). */
export function DdlDocument({ spec, params }: DbDocumentProps<"ddl">) {
  const { client, target } = useHost();
  const projectId = "projectId" in target ? target.projectId : undefined;
  const font = useEditorFont();
  const access = useDbAccessPrompt();
  const ask = useRef(access.resolve);
  ask.current = access.resolve;
  const load = useCallback(async () => {
    if (!client || !projectId) throw new Error("no host connection");
    const worktree = params.worktree ?? null;
    const { connectionId, object } = spec;
    const ddl = await withDbAccess(
      () => client.call("db.schema.ddl", { projectId, worktree, connectionId, object }),
      (error) =>
        ask.current(error, {
          calls: client,
          projectId,
          worktree,
          connectionId,
          connectionName: connectionId,
        }),
    );
    return ddl.text;
  }, [client, projectId, params.worktree, spec]);
  const syntax = useMemo<StaticSyntaxSource | undefined>(
    () =>
      client && {
        open: (p) => client.call("syntax.open", p),
        highlights: (p) => client.call("syntax.highlights", p),
        close: (p) => client.call("syntax.close", p),
      },
    [client],
  );
  return (
    <>
      <DdlView
        spec={spec}
        load={load}
        eager={ddlOpenedHere(spec.tabId)}
        font={font}
        syntax={syntax}
      />
      {access.element}
    </>
  );
}
