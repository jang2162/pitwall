// @vitest-environment happy-dom
// 다른 머신 탭의 여러 파일 편집(18-m7-plan M7-21 리뷰 F1): rename(⇧F6) · 코드 액션(⌥↩)은 창의 이 머신 적용기로 가지 않는다 —
// 판정 한 자리(edit-window.ts `editsHere`)가 서버에 묻기 전에 「아직 없다」 로 그만둔다. 이 머신 탭은 그대로 묻는다(대조).

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";

import { checkoutKey, qualify } from "../checkout/checkout";
import { editorContext } from "../editor/extensions/context";
import { runCodeAction, showCodeActions } from "./code-actions";
import { createEditWindow, type EditWindow } from "./edit-window";
import { createLspSessions, type LspDocument } from "./lsp-sessions";
import { fakeLspDocuments, fakeLspServer } from "./lsp-testing";
import { renameAt } from "./rename";

const MINI = "SHA256:mini/fingerprint";
const PATH = "/work/demo/src/a.rs";
const CAP = { server: "rust-analyzer", options: true, optionsTruncated: false };
const settle = () => new Promise((r) => setTimeout(r, 0));

async function setup() {
  const here = fakeLspServer();
  const mini = fakeLspServer();
  for (const s of [here, mini]) s.setFeatures({ rename: CAP, code_action: CAP } as never);
  const sessions = createLspSessions({
    source: here.source,
    services: here.services,
    documents: fakeLspDocuments({ [PATH]: "fn a() {}", [qualify(MINI, PATH)]: "fn a() {}" }),
    projectId: "p1",
    machine: (m) =>
      m === MINI ? { projectId: "r1", source: mini.source, services: mini.services } : undefined,
  });
  const remote = sessions.acquire(qualify(MINI, PATH), "Rust", checkoutKey({ machine: MINI }));
  const local = sessions.acquire(PATH, "Rust");
  await settle();
  await settle();
  const notify = vi.fn(() => ({ close() {}, update() {} }));
  const begin = vi.fn(() => ({ apply: vi.fn(), fs: {} }));
  const win = {
    strings: () =>
      new Proxy({} as Record<string, unknown>, {
        get: (_t, key) => (key === "remoteEdits" ? "REMOTE" : () => String(key)),
      }),
    notify,
    applier: { begin },
    execute: vi.fn(),
    report: vi.fn(),
  } as unknown as EditWindow;
  const viewOf = (doc: LspDocument) =>
    new EditorView({
      state: EditorState.create({
        doc: "fn a() {}",
        extensions: [editorContext.of({ lsp: doc } as never)],
      }),
    });
  return { here, mini, remote: remote.document, local: local.document, win, notify, begin, viewOf };
}

const asked = (s: ReturnType<typeof fakeLspServer>, method: string) =>
  s.called(`call:${method}`).length;

describe("edits of another machine's tab", () => {
  it("refuses rename and code actions before asking any server, and says so", async () => {
    const fx = await setup();
    expect(fx.remote.capabilities.rename).toBeDefined();
    await renameAt(fx.viewOf(fx.remote), fx.win);
    await showCodeActions(fx.viewOf(fx.remote), fx.win);
    const flow = { apply: vi.fn(), fs: {} };
    await runCodeAction(
      fx.remote,
      fx.win,
      { title: "fix" } as never,
      "rust-analyzer",
      flow as never,
    );
    for (const s of [fx.here, fx.mini]) {
      expect(asked(s, "lsp.rename.prepare")).toBe(0);
      expect(asked(s, "lsp.codeAction.list")).toBe(0);
      expect(asked(s, "lsp.codeAction.resolve")).toBe(0);
    }
    expect(fx.begin).not.toHaveBeenCalled();
    expect(flow.apply).not.toHaveBeenCalled();
    expect(fx.win.execute).not.toHaveBeenCalled();
    expect(fx.notify).toHaveBeenCalledTimes(3);
    expect(fx.notify).toHaveBeenCalledWith({ level: "info", title: "REMOTE" });
  });

  it("still asks for this machine's tab", async () => {
    const fx = await setup();
    await renameAt(fx.viewOf(fx.local), fx.win);
    await showCodeActions(fx.viewOf(fx.local), fx.win);
    expect(asked(fx.here, "lsp.rename.prepare")).toBe(1);
    expect(asked(fx.here, "lsp.codeAction.list")).toBe(1);
    expect(fx.notify).not.toHaveBeenCalledWith({ level: "info", title: "REMOTE" });
  });
});

// 리뷰 반영 2 권고 R4: 명령(완성 명령 · 전구 · 빠른 수정 · 코드 액션의 명령)의 편집도 이 창의 적용기로 온다 — `EditWindow.execute`
// 안에서도 같은 판정을 지난다(나중에 머신마다 EditWindow 를 두어도 그 길이 열리지 않게).
describe("a command of another machine's document", () => {
  it("is not sent from this window's edit context", async () => {
    const fx = await setup();
    const notify = vi.fn(() => ({ close() {}, update() {} }));
    const win = createEditWindow({
      services: fx.here.services,
      strings: () =>
        new Proxy({} as Record<string, unknown>, {
          get: (_t, key) => (key === "remoteEdits" ? "REMOTE" : () => String(key)),
        }) as never,
      notify: notify as never,
      lightbulb: () => true,
      fs: {} as never,
      workspace: {} as never,
      preview: async () => false,
    });
    await win.execute(fx.remote, { command: "x" }, "rust-analyzer");
    for (const s of [fx.here, fx.mini]) expect(asked(s, "lsp.command.execute")).toBe(0);
    expect(notify).toHaveBeenCalledWith({ level: "info", title: "REMOTE" });
    // 대조: 이 머신 문서의 명령은 보낸다.
    fx.here.handle("lsp.command.execute", async () => ({ staleVersion: false }));
    await win.execute(fx.local, { command: "x" }, "rust-analyzer");
    expect(asked(fx.here, "lsp.command.execute")).toBe(1);
    win.dispose();
  });
});
