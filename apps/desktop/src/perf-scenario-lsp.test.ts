// @vitest-environment happy-dom
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { RpcClient } from "@pitwall/rpc-client";
import { afterEach, expect, it } from "vitest";

import { editorStartingWith, MAIN_FIRST_LINE, watchSymbolCalls } from "./perf-scenario-lsp";

afterEach(() => {
  document.body.replaceChildren();
});

it("times index.symbols.search round trips by query and leaves other calls alone", async () => {
  const proto = RpcClient.prototype as unknown as {
    call: (method: string, params?: unknown) => Promise<unknown>;
  };
  const original = proto.call;
  const answers: string[] = [];
  // 이 시험의 「Host」: 메서드 이름을 적고 곧바로 답한다.
  proto.call = function (method: string) {
    answers.push(method);
    return Promise.resolve({ method });
  };
  const stub = proto.call;
  try {
    let clock = 0;
    const calls = watchSymbolCalls(() => clock);
    const client = Object.create(RpcClient.prototype) as RpcClient;
    const pending = client.call("index.symbols.search", { projectId: "p", query: "ha" });
    clock = 12.5;
    await pending;
    await client.call("index.status", { projectId: "p" });
    expect(calls.rpc("ha")).toBe(12.5);
    // 질의가 없는 다른 메서드는 적지 않는다(적었다면 빈 질의 자리에 들었다).
    expect(calls.rpc("")).toBeUndefined();
    expect(answers).toEqual(["index.symbols.search", "index.status"]);
    calls.restore();
    expect(proto.call).toBe(stub);
  } finally {
    proto.call = original;
  }
});

it("finds the visible editor by its first line", () => {
  const mount = (text: string, hidden = false) => {
    const parent = document.createElement("div");
    if (hidden) parent.style.display = "none";
    document.body.append(parent);
    return new EditorView({ state: EditorState.create({ doc: text }), parent });
  };
  mount("// other file\n");
  const main = mount(`${MAIN_FIRST_LINE}\nfn main() {}\n`);
  // happy-dom 은 배치를 하지 않는다 — 보이는지는 getClientRects 로 본다.
  const rects = (n: number) => () => ({ length: n }) as DOMRectList;
  for (const el of document.querySelectorAll<HTMLElement>(".cm-editor"))
    el.getClientRects = rects(1);
  expect(editorStartingWith(document, MAIN_FIRST_LINE)).toBe(main);
  main.dom.getClientRects = rects(0);
  expect(editorStartingWith(document, MAIN_FIRST_LINE)).toBeUndefined();
});
