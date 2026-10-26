// @vitest-environment happy-dom
// 창의 git 질의(docs/design/13-m2-plan.md M2-06 완료 기준 ⑤ 의 창 쪽): 창의 작업 공간(문서 저장소 그대로)에 미저장 편집이 든 문서를
// 두고, Host 가 그 파일을 바꾸는 체크아웃의 미저장 확인을 `git.prompts` 로 보내면 M1-13 의 확인을 묻고 답하기 전에는 Host 에 답하지
// 않는다 — 취소면 `cancel`(체크아웃은 돌지 않는다), 저장이면 저장한 뒤 `proceed`. 겹치지 않는 쓰기는 묻지 않고 `proceed`. askpass
// 질문은 종류(`hint`)와 함께 다이얼로그 자리로, 닫힘은 그 자리를 내린다. Host 쪽(답이 오기 전에 git 이 돌지 않는다)은 crates/pw-git/src/write/tests/unsaved.rs.

import { EditorView } from "@codemirror/view";
import type {
  FsRead,
  FsStat,
  FsWritten,
  GitPromptAnswer,
  GitPromptEvent,
  RpcClient,
} from "@pitwall/rpc-client";
import { expect, it, vi } from "vitest";

import { createDocumentStore, type DocumentSource } from "../editor/documents";
import { createEditorWorkspace, type UnsavedAnswer } from "../editor/editor-workspace";
import { answerUnsaved, hostPath, listenGitPrompts } from "./git-prompts";

const WORK_TREE = "/w";
const README = "/w/README.md";

const flush = () => new Promise((r) => setTimeout(r, 0));

function source(): DocumentSource & { write: ReturnType<typeof vi.fn> } {
  const text = "hello\n";
  return {
    stat: async (path): Promise<FsStat> => ({
      path,
      kind: "file",
      target: null,
      size: text.length,
      mtimeMs: 1,
      readonly: false,
      sizeClass: "editable",
    }),
    read: async ({ path }): Promise<FsRead> => ({
      path,
      size: text.length,
      mtimeMs: 1,
      offset: 0,
      eof: true,
      version: "v1",
      content: { kind: "text", text },
      judgment: {
        binary: false,
        encoding: "UTF-8",
        lossy: false,
        bom: null,
        lineEnding: "lf",
        longLines: false,
        sizeClass: "editable",
        editable: true,
        highlight: true,
      },
    }),
    write: vi.fn(async ({ path }: { path: string }): Promise<FsWritten> => ({
      path,
      size: 13,
      mtimeMs: 2,
      version: "v2",
    })),
  };
}

/** 창의 작업 공간 그대로: `README.md` 를 열어 편집한(미저장) 상태. `answer` 가 M1-13 의 확인에 답한다. */
async function windowWithUnsavedReadme(answer: () => Promise<UnsavedAnswer>) {
  const files = source();
  const documents = createDocumentStore(files);
  const confirm = vi.fn(answer);
  const workspace = createEditorWorkspace({
    documents,
    stat: files.stat,
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: confirm,
  });
  documents.acquire(README);
  await flush();
  const doc = documents.get(README);
  if (doc?.status !== "text") throw new Error("README.md did not open as text");
  const made = doc.live.createState("tab", []);
  const view = new EditorView({
    parent: document.body,
    state: made.state,
    dispatchTransactions: (trs, v) => doc.live.dispatch(trs, v),
  });
  doc.live.attach("tab", view, () => undefined);
  view.dispatch({ changes: { from: 0, insert: "edited " }, userEvent: "input.type" });
  expect(doc.live.dirty).toBe(true);
  return { workspace, confirm, files, view };
}

/** `git.prompts` 를 흉내 낸 클라이언트: 넣은 이벤트를 스트림으로 내고, 답(`git.prompt.answer`)을 모은다. */
function fakeHost() {
  const queue: GitPromptEvent[] = [];
  let wake: (() => void) | undefined;
  const answers: { promptId: number; answer: GitPromptAnswer }[] = [];
  const client = {
    stream(method: string, _params: unknown, options: { signal: AbortSignal }) {
      expect(method).toBe("git.prompts");
      return (async function* () {
        while (!options.signal.aborted) {
          const next = queue.shift();
          if (next) yield next;
          else await new Promise<void>((resolve) => (wake = resolve));
        }
      })();
    },
    call: vi.fn(async (method: string, params: { promptId: number; answer: GitPromptAnswer }) => {
      expect(method).toBe("git.prompt.answer");
      answers.push({ promptId: params.promptId, answer: params.answer });
      return null;
    }),
  };
  return {
    client: client as unknown as RpcClient,
    answers,
    push(event: GitPromptEvent) {
      queue.push(event);
      wake?.();
    },
  };
}

function checkout(promptId: number, paths: string[] | null): GitPromptEvent {
  return {
    kind: "open",
    prompt: { promptId, request: { kind: "unsaved", workTree: WORK_TREE, paths } },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

it("does not let a checkout that changes a file with unsaved edits go ahead until the person confirms", async () => {
  const decision = deferred<UnsavedAnswer>();
  const w = await windowWithUnsavedReadme(() => decision.promise);
  const host = fakeHost();
  const stop = listenGitPrompts(host.client, "p1", {
    unsaved: (request) => answerUnsaved(request, w.workspace),
    askpass() {},
    closed() {},
  });
  host.push(checkout(1, ["README.md"]));
  await flush();
  await flush();
  expect(w.confirm).toHaveBeenCalledWith([README]);
  expect(host.answers).toEqual([]);

  decision.resolve("cancel");
  await vi.waitFor(() =>
    expect(host.answers).toEqual([{ promptId: 1, answer: { kind: "cancel" } }]),
  );
  expect(w.files.write).not.toHaveBeenCalled();
  stop();
  w.view.destroy();
});

it("saves before going ahead when the person picks save", async () => {
  const w = await windowWithUnsavedReadme(async () => "save");
  const host = fakeHost();
  const stop = listenGitPrompts(host.client, "p1", {
    unsaved: (request) => answerUnsaved(request, w.workspace),
    askpass() {},
    closed() {},
  });
  host.push(checkout(2, null));
  await vi.waitFor(() =>
    expect(host.answers).toEqual([{ promptId: 2, answer: { kind: "proceed" } }]),
  );
  expect(w.confirm).toHaveBeenCalledWith([README]);
  expect(w.files.write).toHaveBeenCalledWith(
    expect.objectContaining({ path: README, content: { kind: "text", text: "edited hello\n" } }),
    undefined,
  );
  stop();
  w.view.destroy();
});

it("goes ahead without asking when the write does not touch an unsaved document", async () => {
  const w = await windowWithUnsavedReadme(async () => "cancel");
  const host = fakeHost();
  const stop = listenGitPrompts(host.client, "p1", {
    unsaved: (request) => answerUnsaved(request, w.workspace),
    askpass() {},
    closed() {},
  });
  host.push(checkout(3, ["src/other.rs", "README.md.orig"]));
  await vi.waitFor(() =>
    expect(host.answers).toEqual([{ promptId: 3, answer: { kind: "proceed" } }]),
  );
  expect(w.confirm).not.toHaveBeenCalled();
  stop();
  w.view.destroy();
});

it("hands askpass questions to the dialog and takes them down when they close", async () => {
  const host = fakeHost();
  const opened: unknown[] = [];
  const closed: number[] = [];
  const stop = listenGitPrompts(host.client, "p1", {
    unsaved: async () => ({ kind: "proceed" }),
    askpass: (prompt) => opened.push(prompt),
    closed: (id) => closed.push(id),
  });
  host.push({
    kind: "open",
    prompt: {
      promptId: 4,
      request: { kind: "askpass", text: "Password for 'https://h': ", hint: "secret" },
    },
  });
  host.push({
    kind: "open",
    prompt: {
      promptId: 5,
      request: { kind: "askpass", text: "Confirm user presence", hint: "notice" },
    },
  });
  host.push({ kind: "closed", promptId: 4 });
  host.push({ kind: "closed", promptId: 5 });
  await vi.waitFor(() => expect(closed).toEqual([4, 5]));
  expect(opened).toEqual([
    { promptId: 4, text: "Password for 'https://h': ", hint: "secret" },
    { promptId: 5, text: "Confirm user presence", hint: "notice" },
  ]);
  // 창은 askpass 를 다이얼로그에서만 답한다.
  expect(host.answers).toEqual([]);
  stop();
});

it("joins git paths onto the work tree with the host's separator", () => {
  expect(hostPath("/w", "src/a.rs")).toBe("/w/src/a.rs");
  expect(hostPath("/w/", "a")).toBe("/w/a");
  expect(hostPath(String.raw`C:\w`, "src/a.rs")).toBe(String.raw`C:\w\src\a.rs`);
});
