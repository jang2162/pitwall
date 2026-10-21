// @vitest-environment happy-dom
import { LocalizationProvider } from "@fluent/react";
import type { DockviewApi } from "dockview-react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { bundledLocalization, bundledTranslator, KO_PROJECT } from "../i18n/testing";
import { NotificationProvider } from "../notifications/notifications";
import { LAYOUT_STORAGE_KEY } from "./project-layout";
import { ProjectDock } from "./project-window";
import { projectStrings } from "./strings";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

// 레이아웃만 본다. 파일 트리는 에디터 · 키맵 · Host 를 쓰므로(file-tree.test.tsx 가 본다) 빈 내용으로 바꾼다.
vi.mock("../file-tree/file-tree", () => ({ FileTreeToolWindow: () => null }));

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  vi.restoreAllMocks();
});

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
    removeItem: (key: string) => void items.delete(key),
  };
}

function render(
  storage: ReturnType<typeof memoryStorage>,
  locale: "ko" | "en",
  onApi: (api: DockviewApi) => void,
) {
  const l10n = bundledLocalization(locale);
  const titles = projectStringsFor(locale).toolWindows;
  // 앱처럼 StrictMode 로 렌더한다. StrictMode 는 effect 를 한 번 해제했다 다시 붙인다.
  return root!.render(
    <StrictMode>
      <LocalizationProvider l10n={l10n}>
        <NotificationProvider>
          <ProjectDock storage={storage} titles={titles} onApi={onApi} />
        </NotificationProvider>
      </LocalizationProvider>
    </StrictMode>,
  );
}

const EN_PROJECT = projectStrings(bundledTranslator("en"));
const projectStringsFor = (locale: "ko" | "en") => (locale === "ko" ? KO_PROJECT : EN_PROJECT);

async function mount(storage: ReturnType<typeof memoryStorage>, locale: "ko" | "en" = "ko") {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "760px";
  document.body.append(element);
  let api: DockviewApi | undefined;
  root = createRoot(element);
  await act(async () => render(storage, locale, (a) => (api = a)));
  return api!;
}

it("keeps saving layout changes after StrictMode remounts effects, and restores them", async () => {
  const storage = memoryStorage();
  const api = await mount(storage);
  expect(edge(api).activePanel?.id).toBe("project");

  await act(async () => api.getPanel("structure")!.api.setActive());
  await vi.waitFor(() => expect(storage.items.has(LAYOUT_STORAGE_KEY)).toBe(true), {
    timeout: 2000,
  });

  act(() => root!.unmount());
  root = undefined;
  const restored = await mount(storage);
  expect(edge(restored).activePanel?.id).toBe("structure");
});

/** 왼쪽 툴 윈도우의 그룹. */
function edge(api: DockviewApi) {
  const id = api.getEdgeGroup("left")!.id;
  return api.groups.find((g) => g.id === id)!;
}

function tabOf(title: string) {
  const tab = [...document.querySelectorAll<HTMLElement>("[role=tab]")].find(
    (el) => el.textContent === title,
  );
  if (!tab) throw new Error(`탭 ${title} 이 없다`);
  return tab;
}

it("names the tabs in the UI language and renames them when the language changes", async () => {
  const storage = memoryStorage();
  const api = await mount(storage, "ko");
  expect(tabOf("쿼리 결과")).toBeDefined();
  expect(api.getPanel("terminal")!.title).toBe("터미널");

  await act(async () => render(storage, "en", () => {}));
  expect(tabOf("Query Results")).toBeDefined();
  expect(api.getPanel("terminal")!.title).toBe("Terminal");
});

it("names restored tabs in the current language, not the one they were saved in", async () => {
  const storage = memoryStorage();
  const api = await mount(storage, "ko");
  await act(async () => api.getPanel("structure")!.api.setActive());
  await vi.waitFor(() => expect(storage.items.get(LAYOUT_STORAGE_KEY)).toContain("구조"), {
    timeout: 2000,
  });
  act(() => root!.unmount());
  root = undefined;

  const restored = await mount(storage, "en");
  expect(restored.getPanel("project")!.title).toBe("Project");
  expect(tabOf("Project")).toBeDefined();
});
