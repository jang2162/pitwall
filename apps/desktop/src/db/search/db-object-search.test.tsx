// @vitest-environment happy-dom
// Search Everywhere 의 「DB 객체」 탭(M3-20)을 팝업째 본다. Host 는 가짜 `db.schema.cached` 이고 액션 레지스트리는 진짜다 — 탐색기의
// `db.explorer.reveal` · 테이블 데이터의 `db.tableData.open`(M3-17)은 시험이 등록하거나 빼서 「등록되지 않았으면 끈다」 를 본다.
//
// 본다(14 의 M3-20 완료 기준 「⇧⇧ DB 객체 탭에서 테이블을 찾아 연다」): 탭을 열면 캐시된 이름만 한 번 읽고(연결 · 스키마 조회 RPC
// 가 없다) 이름으로 거른다, Enter 는 탐색기에서 고르고(탐색기가 늦게 등록돼도), ⌘↩ 는 테이블 데이터를 연다 — 등록되지 않았으면
// 아무것도 하지 않는다. 미신뢰 프로젝트는 비운다.

import { LocalizationProvider } from "@fluent/react";
import { ActionRegistry } from "@pitwall/keymap";
import {
  type DbCachedObject,
  type DbSchemaCached,
  ERROR_CODE,
  RpcCallError,
} from "@pitwall/rpc-client";
import { useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import { bundledLocalization } from "../../i18n/testing";
import { type SearchMode, SearchEverywhere } from "../../search-everywhere/search-everywhere";
import { act, endTest, it } from "../../test-support/ended-test";
import { CachedNames, dbObjectSearchSource } from "./db-object-search";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const keymap = vi.hoisted(() => ({
  registry: undefined as unknown as import("@pitwall/keymap").ActionRegistry,
  platform: "mac" as "mac" | "win" | "linux",
}));

vi.mock("../../keymap/keymap-provider", () => ({
  useKeymap: () => ({ registry: keymap.registry, platform: keymap.platform }),
  useRegistryVersion: () => {},
  useSearchSource: () => {},
  useKeymapPopup: () => {},
}));

const cached = (
  connectionId: string,
  name: string,
  kind: "table" | "view" = "table",
  schema = "public",
): DbCachedObject => ({
  connectionId,
  connectionName: connectionId === "pg" ? "billing pg" : connectionId,
  object: { database: "billing", schema, name },
  kind,
});

const host = {
  reads: 0,
  answer: (): Promise<DbSchemaCached> =>
    Promise.resolve({
      items: [
        cached("pg", "customers"),
        cached("pg", "orders"),
        cached("pg", "order_items"),
        cached("pg", "active_orders", "view", "sales"),
        cached("lite", "orders"),
      ],
      truncated: false,
    }),
};

let root: Root | undefined;
let element: HTMLDivElement;
let mode: SearchMode | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  host.reads = 0;
  keymap.registry = new ActionRegistry();
  keymap.platform = "mac";
  element = document.createElement("div");
  document.body.append(element);
});

afterEach(async () => {
  await endTest();
  await act(async () => root?.unmount());
  root = undefined;
  element.remove();
  document.body.innerHTML = "";
});

async function settle() {
  for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
}

async function open(read: () => Promise<DbSchemaCached> = host.answer) {
  const source = dbObjectSearchSource({
    names: new CachedNames(() => {
      host.reads++;
      return read();
    }),
    api: () => undefined,
  });
  function Popup() {
    const [m, setM] = useState<SearchMode | undefined>("dbObjects");
    mode = m;
    return <SearchEverywhere mode={m} onModeChange={setM} sources={[source]} />;
  }
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <LocalizationProvider l10n={bundledLocalization("ko")}>
        <Popup />
      </LocalizationProvider>,
    ),
  );
  await settle();
}

const input = () => document.querySelector<HTMLInputElement>("[cmdk-input]")!;
const names = () =>
  [...document.querySelectorAll<HTMLElement>("[data-db-object]")].map((e) =>
    e.getAttribute("data-db-object"),
  );
const selected = () =>
  document.querySelector<HTMLElement>('[cmdk-item][data-selected="true"]')?.textContent;

async function type(text: string) {
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input(), text);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function key(init: KeyboardEventInit) {
  await act(async () => {
    input().dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

it("reads the cached names once when the tab opens and filters them by name", async () => {
  await open();
  expect(host.reads).toBe(1);
  expect(names()).toEqual(["customers", "orders", "order_items", "active_orders", "orders"]);
  await type("ord");
  expect(host.reads).toBe(1);
  // 이름 앞에서 맞은 것이 먼저, 중간에 맞은 것은 뒤.
  expect(names()).toEqual(["orders", "order_items", "orders", "active_orders"]);
  expect(document.body.textContent).toContain("billing pg · billing · public");
  expect(document.body.textContent).toContain("뷰");
  // 스키마 이름으로도 찾는다(이름으로 맞은 것보다 뒤).
  await type("sales");
  expect(names()).toEqual(["active_orders"]);
});

it("selects the object in the explorer on Enter, even when the explorer registers late", async () => {
  await open();
  await type("order_items");
  const reveal = vi.fn();
  await key({ key: "Enter", code: "Enter" });
  expect(mode).toBeUndefined();
  await act(async () => {
    keymap.registry.register({
      id: "db.explorer.reveal",
      label: "reveal",
      args: true,
      run: reveal,
    });
  });
  await settle();
  expect(reveal).toHaveBeenCalledWith({
    connectionId: "pg",
    object: { database: "billing", schema: "public", name: "order_items" },
    kind: "table",
  });
});

it("opens table data on ⌘↩ (Ctrl+Enter on Windows and Linux), also from a Korean query", async () => {
  for (const platform of ["mac", "win"] as const) {
    keymap.platform = platform;
    keymap.registry = new ActionRegistry();
    const data = vi.fn();
    const reveal = vi.fn();
    keymap.registry.register({ id: "db.tableData.open", label: "data", args: true, run: data });
    keymap.registry.register({ id: "db.explorer.reveal", label: "r", args: true, run: reveal });
    await open();
    await type("customers");
    expect(selected()).toContain(platform === "mac" ? "⌘↩ 데이터" : "Ctrl+Enter 데이터");
    // 조합 중인 키(한글 입력 확정)는 단축키가 아니다 — 같은 수식키라도.
    const openData =
      platform === "mac"
        ? { key: "Enter", code: "Enter", metaKey: true }
        : { key: "Enter", code: "Enter", ctrlKey: true };
    await key({ ...openData, isComposing: true });
    expect(data).not.toHaveBeenCalled();
    await key({ ...openData, keyCode: 229 } as KeyboardEventInit);
    expect(data).not.toHaveBeenCalled();
    expect(mode, platform).toBe("dbObjects");
    await key(openData);
    expect(mode, platform).toBeUndefined();
    await settle();
    expect(data, platform).toHaveBeenCalledWith({
      connectionId: "pg",
      object: { database: "billing", schema: "public", name: "customers" },
    });
    expect(reveal).not.toHaveBeenCalled();
    await act(async () => root?.unmount());
    root = undefined;
  }
});

it("does nothing on ⌘↩ while table data is not registered", async () => {
  const reveal = vi.fn();
  keymap.registry.register({ id: "db.explorer.reveal", label: "r", args: true, run: reveal });
  await open();
  await type("customers");
  expect(selected()).not.toContain("⌘↩");
  await key({ key: "Enter", code: "Enter", metaKey: true });
  expect(mode).toBe("dbObjects");
  await settle();
  expect(reveal).not.toHaveBeenCalled();
});

it("shows nothing for an untrusted project and says why", async () => {
  await open(() =>
    Promise.reject(
      new RpcCallError({
        code: ERROR_CODE.PERMISSION_DENIED,
        message: "untrusted",
        data: { reason: "untrusted", projectId: "p1" },
      }),
    ),
  );
  expect(names()).toEqual([]);
  expect(document.body.textContent).toContain("신뢰하지 않은 프로젝트");
});

it("says how names get there when the cache is empty", async () => {
  await open(() => Promise.resolve({ items: [], truncated: false }));
  expect(names()).toEqual([]);
  expect(document.body.textContent).toContain("DB 탐색기에서 연결을 펼치면");
});
