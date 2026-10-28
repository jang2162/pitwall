// 3-way 머지 문서(에디터 탭, 13-m2-plan M2-19 · 결정 M2-D4, 10-ui.md 「에디터 영역 문서 타입」 의 3-way 머지 행).
//
// - 연 때 `git.conflict.get` 으로 세 쪽 · 자동 병합 결과 · 구간을 읽는다(M2-11). 자동 병합 결과가 없으면(바이너리 · 5 MiB 초과 ·
//   한쪽에서 지움 · 서브모듈 · 심링크) 머지 칸 대신 한쪽 고르기만 보인다.
// - 세 칸: 왼쪽 ours(읽기 전용) · 가운데 결과 · 오른쪽 theirs(읽기 전용). 결과 칸은 코드 에디터(M1-09 의 `EDITOR_FEATURES` —
//   구문 강조 · 찾기 · 편집 키)를 그대로 쓰고, undo 기록 · 구문 세션은 이 칸의 것이다(파일 문서 모델에 들지 않는다). 양옆은
//   리비전 파일과 같은 읽기 전용 강조(M2-15). 세 칸은 구간마다 줄을 맞추고 스크롤을 함께 한다(merge-panes.ts).
// - `clean`(충돌 구간 0개)이어도 index 는 충돌이다 — 고를 충돌 없이 세 칸으로 보이고 저장으로 푼다.
// - 저장(`⌘S` · [저장하고 해결]) = `git.conflict.resolve { content: { text, bom, expectedVersion } }`. `expectedVersion` 은 get 이 준
//   `version` 이다. 그사이 파일이 바뀌어 `CONFLICT` 면 쓰지 않고 다시 읽는다 — 세 쪽이 그대로면 결과 칸은 두고 `version` 만 새로
//   받는다(다시 저장하면 그 파일을 덮는다는 것을 알린다). 고르지 않은 충돌이 남았으면 먼저 묻는다. 풀었으면 탭을 닫는다.
// - BOM 은 ours 쪽에 있었으면 붙인다(병합은 세 쪽 모두 BOM 을 떼고 한다 — M2-11).

import { history, redo, undo } from "@codemirror/commands";
import { Compartment, EditorState, type Text } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  type ViewUpdate,
} from "@codemirror/view";
import type { ConflictPick, GitConflictDetail, Judgment, RpcClient } from "@pitwall/rpc-client";
import { RpcCallError } from "@pitwall/rpc-client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { DocumentChange } from "../../editor/documents";
import { useEditorFont } from "../../editor/editor-font";
import { useEditorWorkspace } from "../../editor/editor-workspace";
import {
  type EditorFeatureContext,
  editorExtensions,
  reconfigureEditor,
} from "../../editor/extensions";
import { editorTheme } from "../../editor/extensions/theme";
import { FindBar, findPanel } from "../../editor/find-bar";
import { createSyntaxSessions, type SyntaxSource } from "../../editor/syntax-sessions";
import { useFileDialogs } from "../../file-tree/file-dialogs";
import { useNotify } from "../../notifications/notifications";
import { useHost } from "../../rpc/host";
import { useTheme } from "../../theme/theme-switcher";
import { GIT_FAILED } from "../branch-ops";
import { revisionHighlight, revisionSyntax } from "../diff/revision-syntax";
import { gitDocumentSource } from "../documents/source";
import {
  applyNonConflicting,
  hasIgnoredNonConflicting,
  lineModel,
  mergeField,
  mergeInput,
  type MergeInput,
  mergeResultExtension,
  type MergeSide,
  regionAction,
  serialize,
  unresolved,
} from "./merge-model";
import {
  alignSpacers,
  compareWithBase,
  type Pane,
  resultPaneExtension,
  setSideStates,
  setSpacers,
  sidePaneExtension,
} from "./merge-panes";
import { isConflict, isGone, mergeSource, pickOnlyReason } from "./source";
import { type GitMergeStrings, useGitMergeStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

type Loaded =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "gone" }
  | { kind: "ready"; detail: GitConflictDetail; language: string | null; seq: number };

/** 탭 하나의 머지 문서. */
export function MergeDocument({
  path,
  worktree,
  panelId,
}: {
  path: string;
  worktree: string | undefined;
  panelId: string;
}) {
  const T = useGitMergeStrings();
  const source = mergeSource((s) => s.source);
  const docs = gitDocumentSource((s) => s.source);
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const seq = useRef(0);

  const load = useCallback(async () => {
    if (!source) return;
    try {
      const detail = await source.conflict(path, worktree);
      let language: string | null = null;
      const sample = detail.ours?.content?.kind === "text" ? detail.ours.content.text : undefined;
      if (docs && sample !== undefined && detail.ours?.judgment?.highlight)
        language = await docs
          .detect(`${detail.workTree}/${path}`, sample.slice(0, 50 * 1024), worktree)
          .catch(() => null);
      setLoaded({ kind: "ready", detail, language, seq: ++seq.current });
    } catch (e) {
      setLoaded(isGone(e) ? { kind: "gone" } : { kind: "failed", message: errorText(e) });
    }
  }, [source, docs, path, worktree]);

  useEffect(() => {
    void load();
  }, [load]);

  if (loaded.kind === "loading") return <Status text={T.doc.loading} />;
  if (loaded.kind === "failed") return <Status text={T.doc.failed(loaded.message)} />;
  if (loaded.kind === "gone") return <Status text={T.doc.gone} />;
  const input = mergeInput(loaded.detail);
  if (!input)
    return <PickOnly detail={loaded.detail} path={path} worktree={worktree} panelId={panelId} />;
  return (
    <MergeView
      key={loaded.seq}
      detail={loaded.detail}
      input={input}
      language={loaded.language}
      path={path}
      worktree={worktree}
      panelId={panelId}
      onGone={() => setLoaded({ kind: "gone" })}
    />
  );
}

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

/** 풀었으면 알리고 탭을 닫는다. */
function useResolved(panelId: string, path: string) {
  const T = useGitMergeStrings();
  const notify = useNotify();
  const workspace = useEditorWorkspace();
  return (left: number) => {
    notify({
      level: "info",
      title: T.doc.saved(path),
      ...(left === 0 && { body: T.doc.allResolved }),
    });
    const panel = workspace.api()?.getPanel(panelId);
    if (panel) void workspace.closeTabs([panel]);
  };
}

/** 자동 병합 결과가 없는 충돌: 한쪽 고르기만. */
function PickOnly({
  detail,
  path,
  worktree,
  panelId,
}: {
  detail: GitConflictDetail;
  path: string;
  worktree: string | undefined;
  panelId: string;
}) {
  const T = useGitMergeStrings();
  const source = mergeSource((s) => s.source);
  const notify = useNotify();
  const resolved = useResolved(panelId, path);
  const [busy, setBusy] = useState(false);
  const reason = pickOnlyReason(detail.file) ?? "other";
  const pick = async (side: ConflictPick) => {
    if (!source) return;
    setBusy(true);
    try {
      const { files } = await source.resolve(
        { path, side: { pick: side, expectedVersion: detail.version } },
        worktree,
      );
      resolved(files.length);
    } catch (e) {
      if (isConflict(e) || isGone(e)) notify({ level: "warning", title: T.conflicts.changed });
      else if (!(e instanceof RpcCallError && e.code === GIT_FAILED))
        notify({ level: "error", title: T.results.failed(T.doc.save), body: errorText(e) });
    } finally {
      setBusy(false);
    }
  };
  const missing = (side: MergeSide) => detail.file[side] === null;
  return (
    <div
      data-git-merge-pick-only={reason}
      className="flex h-full flex-col items-center justify-center gap-3 bg-editor-background px-4 text-center"
    >
      <p className="font-mono text-[12px] text-ui-foreground">{path}</p>
      <p className="text-ui-muted-foreground">
        {T.conflicts.kind(detail.file.kind)} — {T.conflicts.pickOnly(reason)}
      </p>
      <div className="flex gap-2">
        {(["ours", "theirs"] as const).map((side) => (
          <button
            key={side}
            type="button"
            data-git-pick={side}
            className={BUTTON}
            disabled={busy}
            title={missing(side) ? T.doc.sideMissing : undefined}
            onClick={() => void pick(side)}
          >
            {side === "ours" ? T.doc.pickOurs : T.doc.pickTheirs}
          </button>
        ))}
      </div>
    </div>
  );
}

/** 결과 칸 구문 세션이 보는 문서(이 칸 하나). `createSyntaxSessions` 의 문서 모델 자리. */
function resultDocument() {
  let text: Text | undefined;
  const listeners = new Set<(change: DocumentChange) => void>();
  return {
    set(next: Text) {
      text = next;
    },
    emit(change: DocumentChange) {
      text = change.kind === "edit" ? change.after : change.text;
      for (const l of listeners) l(change);
    },
    onChange(_path: string, listener: (change: DocumentChange) => void) {
      listeners.add(listener);
      if (text) listener({ kind: "reset", text });
      return () => void listeners.delete(listener);
    },
  };
}

function hostSyntax(client: () => RpcClient | undefined): SyntaxSource {
  const host = () => {
    const c = client();
    if (!c) throw new Error("Host is not connected");
    return c;
  };
  return {
    open: (p) => host().call("syntax.open", p),
    edit: (p) => host().call("syntax.edit", p),
    highlights: (p) => host().call("syntax.highlights", p),
    close: (p) => host().call("syntax.close", p),
  };
}

const PLAIN_JUDGMENT: Judgment = {
  binary: false,
  encoding: "UTF-8",
  lossy: false,
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

interface Views {
  ours: EditorView;
  result: EditorView;
  theirs: EditorView;
}

function MergeView({
  detail,
  input,
  language,
  path,
  worktree,
  panelId,
  onGone,
}: {
  detail: GitConflictDetail;
  input: MergeInput;
  language: string | null;
  path: string;
  worktree: string | undefined;
  panelId: string;
  onGone(): void;
}) {
  const T = useGitMergeStrings();
  const source = mergeSource((s) => s.source);
  const docs = gitDocumentSource((s) => s.source);
  const workspace = useEditorWorkspace();
  const { client } = useHost();
  const notify = useNotify();
  const dialogs = useFileDialogs();
  const resolved = useResolved(panelId, path);
  const appearance = useTheme((s) => s.appearance);
  const font = useEditorFont();
  const hosts = {
    ours: useRef<HTMLDivElement>(null),
    result: useRef<HTMLDivElement>(null),
    theirs: useRef<HTMLDivElement>(null),
  };
  const views = useRef<Views>(undefined);
  const version = useRef(detail.version);
  const [summary, setSummary] = useState({ left: unresolvedCount(input), ignored: false });
  const [base, setBase] = useState(false);
  const baseOn = useRef(false);
  const [saving, setSaving] = useState(false);
  const [find, setFind] = useState<{ view: EditorView; dom: HTMLElement }>();
  const clientRef = useRef(client);
  clientRef.current = client;

  const createFindPanel = useCallback(
    (view: EditorView) =>
      findPanel(view, setFind, (dom) => setFind((f) => (f?.dom === dom ? undefined : f))),
    [],
  );
  const resultDoc = useMemo(() => resultDocument(), []);
  const syntax = useMemo(
    () =>
      createSyntaxSessions(
        hostSyntax(() => clientRef.current),
        resultDoc,
      ),
    [resultDoc],
  );
  const docKey = `git:merge:${path}`;
  const context = useMemo<EditorFeatureContext>(
    () => ({
      panelId,
      path: docKey,
      judgment: detail.ours?.judgment ?? PLAIN_JUDGMENT,
      readOnly: false,
      language,
      appearance,
      onCursor: (cursor) => workspace.reportCursor(panelId, cursor),
      history: { undo: (view) => undo(view), redo: (view) => redo(view) },
      ...(client && { syntax }),
      font,
      createFindPanel,
    }),
    [
      panelId,
      docKey,
      detail,
      language,
      appearance,
      workspace,
      client,
      syntax,
      font,
      createFindPanel,
    ],
  );
  const latestContext = useRef(context);
  latestContext.current = context;
  const configured = useRef<EditorFeatureContext>(undefined);

  const press = useCallback(
    (index: number, side: MergeSide, action: "apply" | "ignore") => {
      const v = views.current?.result;
      if (!v) return;
      const spec = regionAction(v.state, input, index, side, action);
      if (spec) v.dispatch(spec);
    },
    [input],
  );

  const save = useCallback(async () => {
    const v = views.current?.result;
    if (!v || !source) return;
    const left = unresolved(v.state).length;
    if (left > 0) {
      const ok = await dialogs.confirm({
        title: T.doc.unresolvedTitle(left),
        body: T.doc.unresolvedBody,
        confirm: T.doc.unresolvedConfirm,
      });
      if (!ok) return;
    }
    setSaving(true);
    try {
      const { files } = await source.resolve(
        {
          path,
          content: {
            text: serialize(v.state.doc.toString(), input.result),
            bom: detail.ours?.judgment?.bom === "utf8",
            expectedVersion: version.current,
          },
        },
        worktree,
      );
      resolved(files.length);
    } catch (e) {
      if (isConflict(e)) {
        // 다시 읽는다. 세 쪽이 그대로면 결과 칸은 두고 `version` 만 새로.
        try {
          const again = await source.conflict(path, worktree);
          const same = (["base", "ours", "theirs"] as const).every(
            (s) => again[s]?.id === detail[s]?.id,
          );
          version.current = again.version;
          notify({ level: "warning", title: T.doc.saveConflict });
          if (!same) onGone();
        } catch (again) {
          if (isGone(again)) onGone();
          else
            notify({ level: "error", title: T.results.failed(T.doc.save), body: errorText(again) });
        }
      } else if (isGone(e)) onGone();
      else if (!(e instanceof RpcCallError && e.code === GIT_FAILED))
        notify({ level: "error", title: T.results.failed(T.doc.save), body: errorText(e) });
    } finally {
      setSaving(false);
    }
  }, [source, dialogs, T, path, input, detail, worktree, resolved, notify, onGone]);
  const saveRef = useRef(save);
  saveRef.current = save;

  // 세 칸을 만든다(입력이 바뀌면 문서가 key 로 새로 만든다).
  useEffect(() => {
    const parents = {
      ours: hosts.ours.current,
      result: hosts.result.current,
      theirs: hosts.theirs.current,
    };
    if (!parents.ours || !parents.result || !parents.theirs) return;
    const ctx = latestContext.current;
    const labels = (side: MergeSide) => ({
      apply: side === "ours" ? T.doc.applyOurs : T.doc.applyTheirs,
      ignore: T.doc.ignore,
    });
    const baseCompartments = { ours: new Compartment(), theirs: new Compartment() };
    const sideView = (side: MergeSide) => {
      const raw = detail[side]?.content?.kind === "text" ? detail[side].content.text : "";
      const model = lineModel(raw);
      return new EditorView({
        parent: parents[side]!,
        state: EditorState.create({
          doc: model.doc,
          extensions: [
            lineNumbers(),
            highlightSpecialChars(),
            drawSelection(),
            editorTheme,
            EditorState.readOnly.of(true),
            model.separator ? EditorState.lineSeparator.of(model.separator) : [],
            revisionHighlight(docs?.syntax && revisionSyntax(docs.syntax, language, model.doc)),
            baseCompartments[side].of([]),
            sidePaneExtension(input, side, labels(side), press),
          ],
        }),
      });
    };
    const ours = sideView("ours");
    const theirs = sideView("theirs");

    const refresh = (state: EditorState) => {
      const field = state.field(mergeField);
      for (const side of [ours, theirs]) side.dispatch({ effects: setSideStates.of(field.states) });
      const spacers = baseOn.current
        ? { ours: [], result: [], theirs: [] }
        : alignSpacers(
            input,
            { ours: ours.state.doc, result: state.doc, theirs: theirs.state.doc },
            field.ranges,
          );
      ours.dispatch({ effects: setSpacers.of(spacers.ours) });
      theirs.dispatch({ effects: setSpacers.of(spacers.theirs) });
      return spacers.result;
    };

    const initial = EditorState.create({
      doc: input.result.doc,
      extensions: [
        keymap.of([{ key: "Mod-s", run: () => (void saveRef.current(), true) }]),
        ...editorExtensions(ctx),
        history(),
        input.result.separator ? EditorState.lineSeparator.of(input.result.separator) : [],
        mergeResultExtension(input),
        resultPaneExtension(mergeField),
        EditorView.updateListener.of((u: ViewUpdate) => {
          if (u.docChanged)
            resultDoc.emit({
              kind: "edit",
              changes: u.changes,
              before: u.startState.doc,
              after: u.state.doc,
            });
          if (u.docChanged || u.startState.field(mergeField) !== u.state.field(mergeField)) {
            const spacers = refresh(u.state);
            // 결과 칸의 빈 블록은 이 업데이트가 끝난 뒤에 넣는다(업데이트 안에서 dispatch 하지 않는다).
            queueMicrotask(() => {
              if (views.current?.result !== u.view) return;
              u.view.dispatch({ effects: setSpacers.of(spacers) });
            });
            setSummary({
              left: unresolved(u.state).length,
              ignored: hasIgnoredNonConflicting(u.state),
            });
          }
        }),
      ],
    });
    resultDoc.set(initial.doc);
    const result = new EditorView({ parent: parents.result, state: initial });
    configured.current = ctx;
    result.dispatch({ effects: setSpacers.of(refresh(result.state)) });
    views.current = { ours, result, theirs };
    const unregister = workspace.registerView(panelId, result);

    // 스크롤을 함께 한다.
    const all = [ours, result, theirs];
    let syncing = false;
    const listeners = all.map((from) => {
      const onScroll = () => {
        if (syncing) return;
        syncing = true;
        for (const to of all)
          if (to !== from && to.scrollDOM.scrollTop !== from.scrollDOM.scrollTop)
            to.scrollDOM.scrollTop = from.scrollDOM.scrollTop;
        syncing = false;
      };
      from.scrollDOM.addEventListener("scroll", onScroll);
      return () => from.scrollDOM.removeEventListener("scroll", onScroll);
    });

    baseToggle.current = (on: boolean) => {
      baseOn.current = on;
      for (const [side, view] of [
        ["ours", ours],
        ["theirs", theirs],
      ] as const)
        view.dispatch({
          effects: baseCompartments[side].reconfigure(
            on ? compareWithBase(input, side, lineModel(input.texts.base.join("")).doc) : [],
          ),
        });
      result.dispatch({ effects: setSpacers.of(refresh(result.state)) });
    };

    return () => {
      for (const off of listeners) off();
      unregister();
      baseToggle.current = undefined;
      views.current = undefined;
      for (const v of all) v.destroy();
    };
    // 뷰는 문서(입력)마다 한 번 만든다. 바뀌는 값은 ref · 다시 구성으로 받는다.
  }, [input]);

  const baseToggle = useRef<(on: boolean) => void>(undefined);

  // 설정 · 외관 · 언어가 바뀌면 결과 칸을 다시 구성한다.
  useEffect(() => {
    const v = views.current?.result;
    if (!v || configured.current === context) return;
    v.dispatch(reconfigureEditor(context));
    configured.current = context;
  }, [context]);

  const goConflict = (dir: 1 | -1) => {
    const v = views.current?.result;
    if (!v) return;
    const { ranges } = v.state.field(mergeField);
    const open = unresolved(v.state).map((i) => ranges[i]!.from);
    if (open.length === 0) return;
    const head = v.state.selection.main.head;
    const target =
      dir === 1
        ? (open.find((p) => p > head) ?? open[0]!)
        : ([...open].reverse().find((p) => p < head) ?? open[open.length - 1]!);
    v.dispatch({ selection: { anchor: target }, scrollIntoView: true });
    v.focus();
  };

  return (
    <div data-git-merge={path} className="flex h-full min-h-0 flex-col bg-editor-background">
      <div
        data-git-merge-toolbar=""
        className="flex flex-none flex-wrap items-center gap-2 border-b border-ui-border bg-ui-toolbar px-2 py-0.5 text-[12px]"
      >
        <span data-testid="git-merge-unresolved" className="text-ui-muted-foreground">
          {detail.clean ? T.doc.clean : T.doc.unresolved(summary.left)}
        </span>
        <span className="flex-1" />
        <button
          type="button"
          className={BUTTON}
          disabled={summary.left === 0}
          onClick={() => goConflict(-1)}
        >
          {T.doc.previous}
        </button>
        <button
          type="button"
          className={BUTTON}
          disabled={summary.left === 0}
          onClick={() => goConflict(1)}
        >
          {T.doc.next}
        </button>
        <button
          type="button"
          data-git-merge-apply-all=""
          className={BUTTON}
          disabled={!summary.ignored}
          onClick={() => {
            const v = views.current?.result;
            const spec = v && applyNonConflicting(v.state, input);
            if (v && spec) v.dispatch(spec);
          }}
        >
          {T.doc.applyNonConflicting}
        </button>
        <button
          type="button"
          data-git-merge-base=""
          aria-pressed={base}
          className={`${BUTTON} ${base ? "bg-ui-active" : ""}`}
          onClick={() => {
            const next = !base;
            setBase(next);
            baseToggle.current?.(next);
          }}
        >
          {T.doc.compareBase}
        </button>
        <button
          type="button"
          data-git-merge-save=""
          className={`${BUTTON} border-ui-accent`}
          disabled={saving}
          onClick={() => void save()}
        >
          {T.doc.save}
        </button>
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-3">
        {(["ours", "result", "theirs"] as const satisfies readonly Pane[]).map((pane) => (
          <section
            key={pane}
            aria-label={paneLabel(T, pane)}
            className="flex min-h-0 min-w-0 flex-col border-r border-ui-border last:border-r-0"
          >
            <div className="flex-none truncate border-b border-ui-border px-2 py-0.5 text-[12px] text-ui-muted-foreground">
              {paneLabel(T, pane)}
            </div>
            <div
              ref={hosts[pane]}
              data-git-merge-pane={pane}
              // 결과 칸은 코드 에디터다 — 에디터 키(찾기 · 줄로 이동 …)가 이 칸을 본다.
              {...(pane === "result" && { "data-keymap-context": "editorFocus" })}
              className="min-h-0 flex-1 overflow-hidden select-text"
            />
          </section>
        ))}
      </div>
      {find && createPortal(<FindBar view={find.view} host={find.dom} />, find.dom)}
      {dialogs.element}
    </div>
  );
}

function paneLabel(T: GitMergeStrings, pane: Pane): string {
  return pane === "ours" ? T.doc.ours : pane === "theirs" ? T.doc.theirs : T.doc.result;
}

function unresolvedCount(input: MergeInput): number {
  return input.regions.filter((r) => r.kind === "conflict").length;
}
