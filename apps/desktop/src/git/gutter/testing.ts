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

  async blob(params: { rev?: string | null; path: string; workTreeForm?: boolean }) {
    this.blobCalls.push(params);
    const found = this.files.get(params.path);
    if (found instanceof Error) throw found;
    if (!found) throw Object.assign(new Error("not in HEAD"), { code: -32005 });
    return found;
  }

  async *blame(params: { path: string; rev: string }) {
    this.blameCalls.push(params);
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
}): Harness {
  const path = options.path ?? `${TOP}/a.txt`;
  const source = options.source ?? new FakeSource();
  const ran: { id: string; args: unknown }[] = [];
  const available = new Set(options.available ?? []);
  const host = new GutterHost({
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
