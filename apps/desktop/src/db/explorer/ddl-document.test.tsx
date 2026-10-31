// @vitest-environment happy-dom
// DDL 문서(M3-13 `⌘B`): 이 창에서 연 탭은 곧바로 읽어 읽기 전용으로 보이고, 다시 시작해 되살린 탭은 [DDL 읽기] 를 누를 때만 읽는다
// (결정 M3-D13 — 마운트만으로 연결을 열지 않는다). 같은 객체의 탭은 하나다. 저장본의 꼴은 DB 문서 틀의 검사를 지난다.

import { EditorView } from "@codemirror/view";
import { LocalizationProvider } from "@fluent/react";
import type { IDockviewPanel } from "dockview-react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import { bundledLocalization } from "../../i18n/testing";
import { act, endTest, it } from "../../test-support/ended-test";
import { isDbDocumentSpec } from "../documents/spec";
import { RpcCallError } from "@pitwall/rpc-client";

import { DdlDocument, ddlOpenedHere, DdlView, openDdlDocument } from "./ddl-document";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const tabs = vi.hoisted(() => ({ opened: [] as Record<string, unknown>[] }));

vi.mock("../../editor/editor-tabs", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../editor/editor-tabs")>();
  return {
    ...original,
    openTab: (_api: unknown, options: Record<string, unknown>) => {
      tabs.opened.push(options);
      return { panel: { api: { setActive: vi.fn() } }, created: true };
    },
  };
});

/** `DdlDocument` 가 쓰는 Host(가짜). */
const host = vi.hoisted(() => ({
  calls: [] as { method: string; params: unknown }[],
  answer: (_method: string, _params: unknown): unknown => null,
}));

vi.mock("../../rpc/host", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../rpc/host")>();
  const client = {
    call: async (method: string, params: unknown) => {
      host.calls.push({ method, params });
      return host.answer(method, params);
    },
  };
  return { ...original, useHost: () => ({ client, target: { projectId: "p1" }, key: "test" }) };
});

vi.mock("../../editor/editor-font", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../editor/editor-font")>()),
  useEditorFont: () => ({}),
}));

vi.mock("../../keymap/keymap-provider", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../keymap/keymap-provider")>()),
  useKeymapPopup: () => {},
}));

const SPEC = {
  kind: "ddl" as const,
  tabId: "tab-1",
  connectionId: "pg",
  object: { database: "billing", schema: "public", name: "invoice" },
};

let root: Root | undefined;
let element: HTMLDivElement;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  tabs.opened.length = 0;
  host.calls.length = 0;
  host.answer = () => null;
  element = document.createElement("div");
  document.body.append(element);
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  element.remove();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 0)));

async function mount(eager: boolean, load: () => Promise<string>) {
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <DdlView spec={SPEC} load={load} eager={eager} />
      </LocalizationProvider>,
    ),
  );
  await settle();
}

const editor = () => {
  const dom = document.querySelector<HTMLElement>("[data-db-ddl-text] .cm-editor");
  return dom ? EditorView.findFromDOM(dom) : null;
};

it("reads a tab opened here at once and shows it read-only", async () => {
  const load = vi.fn(async () => "CREATE TABLE invoice (\r\n  id bigint\r\n);");
  await mount(true, load);
  expect(load).toHaveBeenCalledTimes(1);
  const view = editor()!;
  expect(view.state.doc.toString()).toBe("CREATE TABLE invoice (\n  id bigint\n);");
  expect(view.state.readOnly).toBe(true);
  await act(async () =>
    document.querySelector<HTMLElement>("[data-testid=db-ddl-reload]")!.click(),
  );
  await settle();
  expect(load).toHaveBeenCalledTimes(2);
});

it("does not read a restored tab until the person asks", async () => {
  const load = vi.fn(async () => "CREATE VIEW v AS SELECT 1;");
  await mount(false, load);
  expect(load).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain(
    "다시 연 탭이다 — 읽으면 이 연결의 메타데이터용 연결을 연다",
  );
  expect(editor()).toBeNull();
  await act(async () => document.querySelector<HTMLElement>("[data-testid=db-ddl-read]")!.click());
  await settle();
  expect(load).toHaveBeenCalledTimes(1);
  expect(editor()!.state.doc.toString()).toBe("CREATE VIEW v AS SELECT 1;");
});

it("shows a failed read as an alert", async () => {
  await mount(true, async () => {
    throw new Error("permission denied for table invoice");
  });
  expect(document.querySelector("[role=alert]")?.textContent).toBe(
    "읽지 못했다: permission denied for table invoice",
  );
});

it("opens one tab per object and remembers that it was opened here", async () => {
  const existing = {
    params: { path: "db:ddl:old", db: { ...SPEC, tabId: "old" } },
    api: { setActive: vi.fn() },
  } as unknown as IDockviewPanel;
  const other = {
    params: {
      path: "db:ddl:x",
      db: { ...SPEC, tabId: "x", object: { ...SPEC.object, name: "customer" } },
    },
    api: { setActive: vi.fn() },
  } as unknown as IDockviewPanel;
  const panels = [other, existing];
  const workspace = { api: () => ({ panels }) as never, recent: () => [] };

  expect(openDdlDocument(workspace, "pg", SPEC.object, "invoice DDL", undefined)).toBe(existing);
  expect(existing.api.setActive).toHaveBeenCalledTimes(1);
  expect(tabs.opened).toEqual([]);

  openDdlDocument(workspace, "pg", { ...SPEC.object, schema: "sales" }, "invoice DDL", "/wt");
  expect(tabs.opened).toHaveLength(1);
  const opened = tabs.opened[0]!;
  const db = opened.db as typeof SPEC;
  expect(db).toMatchObject({ kind: "ddl", connectionId: "pg", object: { schema: "sales" } });
  expect(opened).toMatchObject({
    path: `db:ddl:${db.tabId}`,
    title: "invoice DDL",
    worktree: "/wt",
  });
  expect(ddlOpenedHere(db.tabId)).toBe(true);
  expect(ddlOpenedHere("restored-tab")).toBe(false);
});

it("keeps only well-formed DDL specs from saved tabs", async () => {
  expect(isDbDocumentSpec(SPEC)).toBe(true);
  expect(isDbDocumentSpec({ ...SPEC, object: { database: null, schema: null, name: "t" } })).toBe(
    true,
  );
  expect(isDbDocumentSpec({ ...SPEC, object: { database: null, schema: null, name: "" } })).toBe(
    false,
  );
  expect(isDbDocumentSpec({ ...SPEC, object: { database: 1, schema: null, name: "t" } })).toBe(
    false,
  );
  expect(isDbDocumentSpec({ ...SPEC, connectionId: "" })).toBe(false);
  expect(isDbDocumentSpec({ ...SPEC, object: "invoice" })).toBe(false);
});

it("asks for the password a restored tab's read needs, and reads again once it is stored", async () => {
  let stored = false;
  host.answer = (method) => {
    if (method === "db.secret.set") {
      stored = true;
      return null;
    }
    if (method === "db.schema.ddl") {
      if (!stored)
        throw new RpcCallError({
          code: 3101,
          message: "db connection `pg` needs its password",
          data: {
            connectionId: "pg",
            key: "password",
            reason: "targetChanged",
            target: {
              driver: "postgres",
              host: "h",
              port: 5432,
              user: "u",
              tunnel: null,
              sslmode: "disable",
              sslrootcert: null,
              sslcert: null,
              sslkey: null,
            },
            previous: null,
          } as never,
        });
      return { text: "CREATE TABLE invoice ();" };
    }
    // 구문 강조(syntax.*)는 이 시험과 상관없다 — 없으면 강조 없이 보인다.
    throw new Error(`unexpected ${method}`);
  };
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <DdlDocument
          panelId="panel-1"
          spec={{ ...SPEC, tabId: "restored" }}
          params={{ worktree: null } as never}
        />
      </LocalizationProvider>,
    ),
  );
  await settle();
  expect(host.calls).toEqual([]);
  await act(async () => document.querySelector<HTMLElement>("[data-testid=db-ddl-read]")!.click());
  await settle();
  const dialog = document.querySelector<HTMLElement>("[data-testid=db-access-prompt]")!;
  expect(dialog.dataset.kind).toBe("secret");
  expect(dialog.textContent).toContain("pg 의 비밀번호");
  const input = dialog.querySelector<HTMLInputElement>("[data-testid=db-access-secret]")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "pw");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => dialog.querySelector<HTMLElement>("[data-action=run]")!.click());
  for (let i = 0; i < 3; i++) await settle();
  expect(host.calls.map((c) => c.method).filter((m) => m.startsWith("db."))).toEqual([
    "db.schema.ddl",
    "db.secret.set",
    "db.schema.ddl",
  ]);
  expect(host.calls[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    connectionId: "pg",
    object: SPEC.object,
  });
  expect(editor()!.state.doc.toString()).toBe("CREATE TABLE invoice ();");
  expect(document.querySelector("[data-testid=db-access-prompt]")).toBeNull();
});
