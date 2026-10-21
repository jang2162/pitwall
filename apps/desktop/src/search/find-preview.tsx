// 전체에서 찾기 팝업 아래의 읽기 전용 미리보기: 고른 결과의 파일을 디스크에서 읽어(`fs.read`, 그 검색의 워크트리 범위) 고른 줄을
// 가운데에 두고, 그 파일의 일치 범위를 모두 칠한다. 에디터 탭의 문서 모델(editor/documents.ts)은 쓰지 않는다 — 고를 때마다 탭 ·
// 문서를 만들지 않게. 앞 [`PREVIEW_READ_MAX`] 만 읽고, 고른 줄이 그 밖이면 결과의 줄 미리보기만 보인다.

import { EditorState, RangeSetBuilder, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  highlightActiveLine,
  lineNumbers,
} from "@codemirror/view";
import type { RpcClient, SearchFile } from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef, useState } from "react";

import { CODE_FONT } from "../editor/extensions/theme";
import type { FindRow } from "./find-session";
import type { FindStrings } from "./strings";

/** 미리보기로 읽는 앞부분(바이트). 이보다 큰 파일의 뒤쪽 결과는 줄 미리보기만 보인다. */
export const PREVIEW_READ_MAX = 1024 * 1024;
/** 고른 줄이 바뀐 뒤 이만큼 멈춰야 읽는다 — ↑↓ 를 누르고 있는 동안 파일마다 읽지 않게. */
const READ_DELAY_MS = 60;

const setMatches = StateEffect.define<DecorationSet>();

const matchField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update: (value, tr) => {
    for (const e of tr.effects) if (e.is(setMatches)) return e.value;
    return value.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

const matchMark = Decoration.mark({ class: "cm-pw-find-match" });

const previewTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--pw-editor-background)",
    color: "var(--pw-editor-foreground)",
    fontSize: "12px",
  },
  ".cm-scroller": { fontFamily: CODE_FONT, lineHeight: "1.5" },
  ".cm-gutters": {
    backgroundColor: "var(--pw-editor-background)",
    color: "var(--pw-ui-mutedForeground)",
    border: "none",
  },
  ".cm-activeLine": { backgroundColor: "var(--pw-ui-hoverBackground)" },
  ".cm-pw-find-match": {
    backgroundColor: "color-mix(in srgb, var(--pw-ui-accent) 30%, transparent)",
    borderRadius: "2px",
  },
});

/** 파일의 결과 범위 → 문서 위치의 칠하기. 읽은 앞부분 밖의 줄은 버린다. */
export function matchDecorations(state: EditorState, file: SearchFile): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const doc = state.doc;
  for (const line of file.lines) {
    if (line.line > doc.lines) break;
    const at = doc.line(line.line);
    for (const r of line.ranges) {
      const from = at.from + Math.min(r.start, at.length);
      const to = at.from + Math.min(r.end, at.length);
      if (to > from) builder.add(from, to, matchMark);
    }
  }
  return builder.finish();
}

type Loaded = { path: string; text: string; complete: boolean } | { path: string; error: string };

export function FindPreview({
  client,
  projectId,
  worktree,
  row,
  strings,
}: {
  client: RpcClient | undefined;
  projectId: string | undefined;
  worktree: string | null | undefined;
  row: FindRow | undefined;
  strings: FindStrings;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView>(undefined);
  const [loaded, setLoaded] = useState<Loaded>();
  const path = row?.file.path;

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          matchField,
          previewTheme,
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = undefined;
    };
  }, []);

  // 파일이 바뀌면 읽는다. 같은 파일 안에서 줄만 바뀌면 다시 읽지 않는다.
  useEffect(() => {
    if (path === undefined || !client || projectId === undefined) return;
    let current = true;
    const timer = setTimeout(() => {
      client
        .call("fs.read", { projectId, worktree: worktree ?? null, path, length: PREVIEW_READ_MAX })
        .then((read) => {
          if (!current) return;
          if (read.content.kind !== "text") setLoaded({ path, error: read.content.kind });
          else setLoaded({ path, text: read.content.text, complete: read.eof });
        })
        .catch((e: unknown) => {
          if (current) setLoaded({ path, error: e instanceof Error ? e.message : String(e) });
        });
    }, READ_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [client, projectId, worktree, path]);

  const ready = loaded && "text" in loaded && loaded.path === path ? loaded : undefined;
  const readLines = useMemo(() => (ready ? countLines(ready) : 0), [ready]);
  // 고른 줄이 읽은 앞부분 밖이면 줄 미리보기만 보인다(아래 대체 줄).
  const inside = row !== undefined && row.line.line <= readLines;

  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.dispatch({
      changes: { from: 0, to: v.state.doc.length, insert: ready?.text ?? "" },
      effects: setMatches.of(Decoration.none),
    });
  }, [ready]);

  // 그 파일의 일치를 칠하고 고른 줄로 간다.
  useEffect(() => {
    const v = view.current;
    if (!v || !ready || !row || row.file.path !== ready.path) return;
    v.dispatch({ effects: setMatches.of(matchDecorations(v.state, row.file)) });
    if (row.line.line > v.state.doc.lines) return;
    const line = v.state.doc.line(row.line.line);
    const pos = line.from + Math.min(row.line.ranges[0]?.start ?? 0, line.length);
    v.dispatch({
      selection: { anchor: pos },
      effects: EditorView.scrollIntoView(pos, { y: "center" }),
    });
  }, [ready, row]);

  const failed = loaded && "error" in loaded && loaded.path === path ? loaded.error : undefined;
  return (
    <div
      aria-label={strings.preview}
      data-testid="find-preview"
      className="relative h-[200px] flex-none border-t border-ui-border"
    >
      <div ref={host} className={`h-full ${inside ? "" : "invisible"}`} />
      {row && !inside && (
        <p className="absolute inset-0 truncate p-2 text-[12px] text-ui-muted-foreground">
          {failed !== undefined
            ? strings.previewFailed(failed)
            : ready
              ? `${row.line.line}: ${row.line.preview}`
              : strings.previewLoading}
        </p>
      )}
    </div>
  );
}

function countLines(loaded: { text: string; complete: boolean }): number {
  // 다 읽지 못했으면 마지막 줄은 잘렸을 수 있어 세지 않는다.
  const lines = loaded.text.split(/\r\n|\r|\n/).length;
  return loaded.complete ? lines : lines - 1;
}
