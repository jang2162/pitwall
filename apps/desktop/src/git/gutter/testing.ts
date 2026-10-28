// 시험용: 거터 · blame 의 가짜 소스와 그 소스로 만든 에디터 뷰(gutter-window.test.tsx · 측정 시험이 함께 쓴다).

import { EditorView } from "@codemirror/view";
import type {
  BlameChunk,
  DiffFile,
  GitBlob,
  HunkSelection,
  Judgment,
  LineEnding,
} from "@pitwall/rpc-client";

import { editorExtensions, type EditorFeatureContext } from "../../editor/extensions";
import { LiveText } from "../../editor/live-text";
import { GutterHost, type GutterHostDeps } from "./gutter-host";
import type { GutterSource } from "./gutter-model";
import { gitGutterStrings } from "./strings";

export const TOP = "/repo";

export function judgment(
  lineEnding: LineEnding = "lf",
  sizeClass: Judgment["sizeClass"] = "editable",
): Judgment {
  return {
    binary: false,
    encoding: "UTF-8",
    lossy: false,
    bom: null,
    lineEnding,
    longLines: false,
    sizeClass,
    editable: sizeClass === "editable",
    highlight: sizeClass === "editable",
  };
}

export function textBlob(text: string): GitBlob {
  return {
    id: "0".repeat(40),
    mode: "100644",
    size: text.length,
    tooLarge: false,
    offset: 0,
    eof: true,
    content: { kind: "text", text },
    judgment: judgment(),
    filter: null,
  };
}

/** 가짜 소스: HEAD 의 파일 내용(작업 트리 꼴) · blame 청크 · 작업 트리 diff 를 시험이 정한다. 부른 것을 적는다. */
export class FakeSource implements GutterSource {
  heads: string | null | undefined = "a".repeat(40);
  /** 저장소 기준 경로 → HEAD 의 내용(`git.blob` 의 응답). 없으면 NOT_FOUND. */
  files = new Map<string, GitBlob | Error>();
  letters = new Map<string, string>();
  blames = new Map<string, BlameChunk[]>();
  diffs = new Map<string, DiffFile>();
  staged: { path: string; hunks: HunkSelection[] }[] = [];
  blobCalls: { rev?: string | null; path: string; workTreeForm?: boolean }[] = [];
  blameCalls: { path: string; rev: string }[] = [];
  private readonly listeners = new Set<() => void>();

  relative(path: string) {
    return path.startsWith(`${TOP}/`) ? path.slice(TOP.length + 1) : undefined;
  }

  letter(path: string) {
    return this.letters.get(path);
  }

  head() {
    return this.heads;
  }

  subscribe(listener: () => void) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** HEAD · 상태가 바뀌었다(감시 이벤트). */
  changed() {
    for (const l of this.listeners) l();
  }

  /** 저장소 꼴(`workTreeForm` 없이)의 blob: `head:<경로>` · `index:<경로>`. 없으면 `files` 의 것. */
  stored = new Map<string, GitBlob>();

  async blob(params: { rev?: string | null; path: string; workTreeForm?: boolean }) {
    this.blobCalls.push(params);
    const raw = params.workTreeForm
      ? undefined
      : this.stored.get(`${params.rev === null ? "index" : "head"}:${params.path}`);
    const found = raw ?? this.files.get(params.path);
    if (found instanceof Error) throw found;
    if (!found) throw Object.assign(new Error("not in HEAD"), { code: -32005 });
    return found;
  }

  /** 이 경로의 blame 은 끝나지 않는다(첫 청크도 없다) — 끊길 때(`signal`)까지 기다린다. 끊긴 경로를 `aborted` 에 적는다. */
  hang = new Set<string>();
  aborted: string[] = [];

  async *blame(params: { path: string; rev: string }, signal?: AbortSignal) {
    this.blameCalls.push(params);
    if (this.hang.has(params.path)) {
      await new Promise<void>((resolve) => {
        if (signal?.aborted) resolve();
        signal?.addEventListener("abort", () => resolve());
      });
      this.aborted.push(params.path);
      return;
    }
    for (const chunk of this.blames.get(params.path) ?? []) yield chunk;
  }

  async workTreeDiff(path: string) {
    return this.diffs.get(path);
  }

  async stageLines(path: string, hunks: HunkSelection[]) {
    this.staged.push({ path, hunks });
  }
}

export interface Harness {
  view: EditorView;
  live: LiveText;
  host: GutterHost;
  source: FakeSource;
  ran: { id: string; args: unknown }[];
  destroy(): void;
}

/** `path`(절대 경로)의 문서를 가짜 소스의 호스트로 연 뷰. */
export function openEditor(options: {
  path?: string;
  text: string;
  lineEnding?: LineEnding;
  source?: FakeSource;
  available?: readonly string[];
  versions?: GutterHostDeps["versions"];
  sizeClass?: Judgment["sizeClass"];
  readOnly?: boolean;
  /** 다른 뷰의 호스트를 함께 쓴다(창 하나의 여러 탭). */
  host?: GutterHost;
}): Harness {
  const path = options.path ?? `${TOP}/a.txt`;
  const source = options.source ?? new FakeSource();
  const ran: { id: string; args: unknown }[] = [];
  const available = new Set(options.available ?? []);
  const host =
    options.host ??
    new GutterHost({
      source,
      strings: gitGutterStrings((id, args) => (args ? `${id} ${JSON.stringify(args)}` : id)),
      available: (id) => available.has(id),
      run: (id, args) => {
        ran.push({ id, args });
        return available.has(id);
      },
      versions: options.versions ?? (async () => ({ document: "v1", disk: "v1" })),
      notify() {},
      locale: "en",
    });
  const lineEnding = options.lineEnding ?? "lf";
  const live = new LiveText({ text: options.text, lineEnding, bom: false });
  const context: EditorFeatureContext = {
    panelId: "editor-1",
    path,
    judgment: judgment(lineEnding, options.sizeClass),
    readOnly: options.readOnly ?? false,
    language: null,
    appearance: "light",
    onCursor: () => {},
    history: live,
    font: {},
    createFindPanel: () => ({ dom: document.createElement("div"), top: true }),
    git: { host, live },
  };
  const made = live.createState("editor-1", editorExtensions(context));
  const parent = document.createElement("div");
  document.body.append(parent);
  const view = new EditorView({
    parent,
    state: made.state,
    dispatchTransactions: (trs, v) => live.dispatch(trs, v),
  });
  const detach = live.attach("editor-1", view, () => undefined);
  return {
    view,
    live,
    host,
    source,
    ran,
    destroy() {
      detach();
      view.destroy();
      parent.remove();
    },
  };
}

/**
 * `git diff -U3` 의 hunk 글(`@@` 줄부터)을 `git.diff` 의 파일로 — 시험에 진짜 git 의 출력을 박아 쓴다. `\ No newline at end of file`
 * 은 앞 줄의 `noNewline` 이다.
 */
export function unifiedFile(patch: string): Pick<DiffFile, "hunks"> {
  const hunks: DiffFile["hunks"] = [];
  let oldLine = 0;
  let newLine = 0;
  for (const raw of patch.split("\n")) {
    const head = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(raw);
    if (head) {
      const [, os, ol, ns, nl] = head;
      hunks.push({
        oldStart: Number(os),
        oldLines: ol === undefined ? 1 : Number(ol),
        newStart: Number(ns),
        newLines: nl === undefined ? 1 : Number(nl),
        header: "",
        lines: [],
      });
      oldLine = Number(os);
      newLine = Number(ns);
      if (ol === "0") oldLine++;
      if (nl === "0") newLine++;
      continue;
    }
    const h = hunks[hunks.length - 1];
    if (!h || raw === "") continue;
    const text = raw.slice(1);
    if (raw[0] === "\\") {
      h.lines[h.lines.length - 1]!.noNewline = true;
    } else if (raw[0] === " ")
      h.lines.push({
        kind: "context",
        text,
        oldLine: oldLine++,
        newLine: newLine++,
        noNewline: false,
      });
    else if (raw[0] === "-")
      h.lines.push({ kind: "removed", text, oldLine: oldLine++, newLine: null, noNewline: false });
    else if (raw[0] === "+")
      h.lines.push({ kind: "added", text, oldLine: null, newLine: newLine++, noNewline: false });
  }
  return { hunks };
}
