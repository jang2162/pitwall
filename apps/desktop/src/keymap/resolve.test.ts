import {
  formatSequence,
  parseKeymapToml,
  type Platform,
  readDefaultKeymap,
  readDefaultKeymapWithWarnings,
  reservedReason,
} from "@pitwall/keymap";
import type { ConfigView } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { createDocumentStore } from "../editor/documents";
import { editorActions } from "../editor/editor-actions";
import { editorOpsActions } from "../editor/editor-ops";
import { createEditorWorkspace } from "../editor/editor-workspace";
import { navigationActions } from "../editor/navigation-ui";
import { lspEditActions } from "../lsp/edit-ui";
import { lspNavigationActions } from "../lsp/navigation-host";
import { fileStructureActions } from "../structure/file-structure-popup";
import { structureStrings } from "../structure/strings";
import { fileOpsActions } from "../file-tree/file-ops";
import { fileTreeStrings } from "../file-tree/strings";
import { explorerActions } from "../db/explorer/explorer-feature";
import { dbExplorerStrings } from "../db/explorer/strings";
import { consoleActions } from "../db/console/console-feature";
import { dbConsoleStrings } from "../db/console/strings";
import { dbTableDataStrings } from "../db/table-data/strings";
import { tableDataActions } from "../db/table-data/table-data-feature";
import { gridActions } from "../db/grid/db-grid-feature";
import { dbGridStrings } from "../db/grid/strings";
import { commitActions } from "../git/commit/commit-feature";
import { remoteActions } from "../git/dialogs/git-remote-feature";
import { branchActions } from "../git/git-branch-feature";
import { gitDocumentActions } from "../git/documents/git-documents";
import { fileHistoryAction } from "../git/history/git-history";
import { gitHistoryStrings } from "../git/history/strings";
import { gitGutterActions } from "../git/gutter/git-gutter";
import { gitGutterStrings } from "../git/gutter/strings";
import { diagnosticActions } from "../lsp/problems";
import { FileTreeModel } from "../file-tree/tree-model";
import { openWithActions } from "../open-with/open-with";
import { findActions } from "../search/find-actions";
import { projectSearchActions } from "../search-everywhere/project-search";
import { terminalActions } from "../terminal/terminal-actions";
import { TerminalTabs } from "../terminal/terminal-tabs";
import { createProjectActions, welcomeActions } from "../welcome/actions";
import { appActions, toolWindowActions } from "./actions";
import { DEFAULT_KEYMAP_TOML } from "./default-keymap";
import { sharesKey } from "./keymap-provider";
import { resolveKeymap } from "./resolve";
import { bundledTranslator, KO_APP, KO_PROJECT, KO_WELCOME } from "../i18n/testing";

const PLATFORMS: Platform[] = ["mac", "win", "linux"];
const defaults = (platform: Platform) =>
  readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform);

const ACTION_IDS = new Set(
  [
    ...appActions({
      strings: KO_APP,
      languages: [],
      setLanguage: async () => {},
      openFindAction() {},
      openSearchEverywhere() {},
      openSettings: async () => {},
      setTheme() {},
    }),
    ...toolWindowActions(() => undefined, KO_PROJECT, {
      focusEditor: { enabled: () => true, run() {} },
    }),
    ...welcomeActions({
      strings: KO_WELCOME,
      platform: "mac",
      selected: () => undefined,
      searchEmpty: () => true,
      writable: () => true,
      newProject() {},
      openFolder: async () => {},
      open() {},
      reveal() {},
      remove() {},
    }),
    ...createProjectActions({ strings: KO_WELCOME, canSubmit: () => true, submit() {} }),
    ...openWithActions({
      strings: KO_APP.openWith,
      enabled: () => true,
      openPopup() {},
      openDefault() {},
    }),
    ...editorActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.editor,
      openSwitcher() {},
      closeWindow: async () => {},
      copyText: async () => {},
      onCopyFailed() {},
    }),
    ...fileOpsActions({
      strings: fileTreeStrings((id) => id).actions,
      model: new FileTreeModel({ list: () => Promise.reject(new Error("none")) }),
      ops: { create: async () => undefined, rename: async () => undefined, remove: async () => {} },
      canOpenTerminal: () => true,
      openInTerminal() {},
    }),
    ...editorOpsActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.editor.actions,
      openGotoLine() {},
      lineComment: () => undefined,
    }),
    ...navigationActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.editor.navigation,
      openRecentLocations() {},
    }),
    ...lspEditActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.editor.lspEdit.actions,
      window: () => undefined,
    }),
    ...lspNavigationActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.editor.lsp,
    }),
    ...fileStructureActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: structureStrings(bundledTranslator("ko")),
      open() {},
    }),
    ...diagnosticActions({
      workspace: createEditorWorkspace({
        documents: createDocumentStore({
          stat: () => Promise.reject(new Error("none")),
          read: () => Promise.reject(new Error("none")),
        }),
        stat: () => Promise.reject(new Error("none")),
        onOpenFailed() {},
        onSaveFailed() {},
        onSaveBlocked() {},
        confirmUnsaved: async () => "cancel",
      }),
      strings: KO_PROJECT.diagnostics,
    }),
    ...terminalActions({
      strings: KO_PROJECT.terminal,
      tabs: new TerminalTabs(),
      canOpen: () => true,
      focusActive() {},
      startRename() {},
      writeClipboard: async () => {},
      readClipboard: async () => "",
      onClipboardFailed() {},
    }),
    ...findActions({ strings: { action: "find" }, open() {} }),
    ...projectSearchActions({
      strings: KO_APP.actions,
      openSearch() {},
      canSwitchProjects: true,
    }),
    ...gitGutterActions({
      view: () => undefined,
      strings: gitGutterStrings((id) => id),
      openBlame() {},
    }),
    ...branchActions({
      strings: { branches: "branches", vcsOperations: "vcs" },
      openBranches() {},
      openVcs() {},
    }),
    ...remoteActions({
      strings: { push: "push", pull: "pull", fetch: "fetch" },
      openPush() {},
      openPull() {},
      fetch() {},
      offer() {},
    }),
    ...gitDocumentActions({
      source: () => undefined,
      open() {},
      strings: KO_PROJECT.git.documents,
      fail() {},
    }),
    ...commitActions({
      strings: { open: "commit", run: "run", runAndPush: "push", rollback: "rollback" },
      feature: () => undefined,
      open() {},
      activeFile: () => undefined,
    }),
    fileHistoryAction({
      strings: () => gitHistoryStrings((id) => id),
      ready: () => true,
      current: () => undefined,
      nested: () => undefined,
      workTree: async () => "/",
      open() {},
      notify() {},
    }),
    ...gridActions(dbGridStrings((id) => id)),
    ...explorerActions({
      strings: dbExplorerStrings((id) => id),
      model: { selected: () => undefined, node: () => undefined },
      openDdl() {},
    }),
    ...tableDataActions({
      strings: dbTableDataStrings((id) => id).actions,
      activeTab: () => undefined,
      open() {},
    }),
    ...consoleActions({
      strings: dbConsoleStrings((id) => id).actions,
      activeTab: () => undefined,
      open() {},
    }),
  ].map((a) => a.id),
);

/**
 * OS 가 가져갈 수 있는데도 일부러 둔 기본 키. macOS 의 `⌃Space` 는 입력 소스가 둘 이상일 때만 OS 가 가져간다 — 자동완성은 IntelliJ 의
 * 그 키와 두 번째 키 `⌥Space` 를 함께 둔다(10-ui §4.2 「코드 (LSP · 편집)」, 15-m4-plan §5 #13). 입력 소스가 하나면 `⌃Space` 가 앱에 온다.
 */
const RESERVED_ON_PURPOSE = new Set(["mac editor.completion ctrl+space"]);

describe("default keymap file", () => {
  it.each(PLATFORMS)("reads on %s, binds only actions that exist, and no OS-reserved key", (p) => {
    const { bindings, warnings } = readDefaultKeymapWithWarnings(
      parseKeymapToml(DEFAULT_KEYMAP_TOML),
      p,
    );
    expect(warnings).toEqual([]);
    expect(bindings.length).toBeGreaterThan(0);
    for (const b of bindings) {
      expect(ACTION_IDS, b.action).toContain(b.action);
      if (RESERVED_ON_PURPOSE.has(`${p} ${b.action} ${b.key}`)) continue;
      expect(reservedReason(b.sequence, p), `${b.action} ${b.key}`).toBeUndefined();
    }
  });

  it("follows the IntelliJ keys of 10-ui §4.2", () => {
    const label = (p: Platform, action: string) =>
      defaults(p)
        .filter((b) => b.action === action)
        .map((b) => formatSequence(b.sequence, p));
    expect(label("mac", "action.find")).toEqual(["⇧⌘A"]);
    expect(label("win", "action.find")).toEqual(["Ctrl+Shift+A"]);
    expect(label("mac", "toolWindow.project")).toEqual(["⌘1"]);
    expect(label("linux", "toolWindow.project")).toEqual(["Alt+1"]);
    expect(label("mac", "toolWindow.terminal")).toEqual(["⌥F12"]);
    expect(label("mac", "settings.open")).toEqual(["⌘,"]);
    // 검색 · 이동(M1-19)
    expect(label("mac", "search.everywhere")).toEqual(["⇧⇧"]);
    expect(label("win", "search.everywhere")).toEqual(["Shift Shift"]);
    expect(label("mac", "search.gotoFile")).toEqual(["⇧⌘O"]);
    expect(label("win", "search.gotoFile")).toEqual(["Ctrl+Shift+N"]);
    expect(label("mac", "search.recentFiles")).toEqual(["⌘E"]);
    expect(label("linux", "search.recentFiles")).toEqual(["Ctrl+E"]);
    expect(label("mac", "project.switcher")).toEqual(["⇧⌘P"]);
    expect(label("win", "project.switcher")).toEqual(["Ctrl+Shift+P"]);
    expect(label("win", "settings.open")).toEqual(["Ctrl+Alt+S"]);
    // 뒤로 · 앞으로 · 최근 위치(M4-09)
    expect(label("mac", "navigate.back")).toEqual(["⌘["]);
    expect(label("mac", "navigate.forward")).toEqual(["⌘]"]);
    expect(label("win", "navigate.back")).toEqual(["Ctrl+Alt+←"]);
    // 자동완성 · 시그니처 도움말(M4-11) — macOS 의 자동완성은 ⌃Space 와 ⌥Space 둘이다.
    expect(label("mac", "editor.completion")).toEqual(["⌃Space", "⌥Space"]);
    expect(label("win", "editor.completion")).toEqual(["Ctrl+Space"]);
    expect(label("linux", "editor.completion")).toEqual(["Ctrl+Space"]);
    expect(label("mac", "editor.parameterInfo")).toEqual(["⌘P"]);
    expect(label("win", "editor.parameterInfo")).toEqual(["Ctrl+P"]);
    expect(label("linux", "navigate.forward")).toEqual(["Ctrl+Alt+→"]);
    expect(label("mac", "navigate.recentLocations")).toEqual(["⇧⌘E"]);
    expect(label("win", "navigate.recentLocations")).toEqual(["Ctrl+Shift+E"]);
    expect(label("mac", "navigate.fileStructure")).toEqual(["⌘F12"]);
    expect(label("linux", "navigate.fileStructure")).toEqual(["Ctrl+F12"]);
    // 10-ui 「툴 윈도우 · 창」 의 레이아웃 프리셋(M2-21 · M3-20)
    expect(label("mac", "layout.preset.default")).toEqual(["⌃⌥1"]);
    expect(label("mac", "layout.preset.gitReview")).toEqual(["⌃⌥2"]);
    expect(label("mac", "layout.preset.db")).toEqual(["⌃⌥3"]);
    expect(label("win", "layout.preset.db")).toEqual(["Ctrl+Alt+3"]);
    expect(label("linux", "layout.preset.db")).toEqual(["Ctrl+Alt+3"]);
    // 10-ui 「외부 도구 · 프로젝트」
    expect(label("mac", "openWith.popup")).toEqual(["⌥F1"]);
    // 10-ui 「Git (VCS)」 의 diff 에서 다음/이전 차이(M2-15)
    expect(label("mac", "diff.nextChange")).toEqual(["F7"]);
    expect(label("win", "diff.previousChange")).toEqual(["Shift+F7"]);
    // 10-ui 「Git (VCS)」 의 푸시 · 업데이트(M2-20)
    expect(label("mac", "git.push.open")).toEqual(["⇧⌘K"]);
    expect(label("win", "git.push.open")).toEqual(["Ctrl+Shift+K"]);
    expect(label("mac", "git.pull.open")).toEqual(["⌘T"]);
    expect(label("linux", "git.pull.open")).toEqual(["Ctrl+T"]);
    expect(label("win", "openWith.popup")).toEqual(["Alt+F1"]);
    expect(label("mac", "openWith.default")).toEqual(["⌥⌘E"]);
    expect(label("linux", "openWith.default")).toEqual(["Ctrl+Alt+E"]);
    // 10-ui 「툴 윈도우 · 창」 · 「검색 · 이동」 — ⌘⌥W 는 Win/Linux 키가 없다(표의 「—」)
    // macOS 의 ⌘W 는 탭이 있으면 탭, 없으면 창을 닫는 `window.close` 다(10-ui §5.2).
    expect(label("mac", "window.close")).toEqual(["⌘W"]);
    expect(label("win", "window.close")).toEqual([]);
    expect(label("mac", "editor.closeTab")).toEqual([]);
    expect(label("win", "editor.closeTab")).toEqual(["Ctrl+F4"]);
    expect(label("mac", "editor.closeOtherTabs")).toEqual(["⌥⌘W"]);
    expect(label("win", "editor.closeOtherTabs")).toEqual([]);
    expect(label("mac", "editor.splitRight")).toEqual(["⌘\\"]);
    expect(label("linux", "editor.splitRight")).toEqual(["Ctrl+\\"]);
    expect(label("mac", "search.findInFiles")).toEqual(["⇧⌘F"]);
    expect(label("win", "search.findInFiles")).toEqual(["Ctrl+Shift+F"]);
    expect(label("linux", "search.findInFiles")).toEqual(["Ctrl+Shift+F"]);
    expect(label("mac", "editor.switcher")).toEqual(["⌃⇥"]);
    expect(label("win", "editor.switcher")).toEqual(["Ctrl+Tab"]);
    // 10-ui 「툴 윈도우 · 창」 의 레이아웃 액션(M1-25)
    expect(label("mac", "toolWindow.hideAll")).toEqual(["⇧⌘F12"]);
    expect(label("win", "toolWindow.hideAll")).toEqual(["Ctrl+Shift+F12"]);
    expect(label("mac", "toolWindow.last")).toEqual(["F12"]);
    expect(label("linux", "toolWindow.last")).toEqual(["F12"]);
    expect(label("mac", "editor.focus")).toEqual(["⎋"]);
    expect(label("win", "editor.focus")).toEqual(["Esc"]);
    expect(label("mac", "toolWindow.stretchLeft")).toEqual(["⌃⌥⇧←"]);
    expect(label("mac", "toolWindow.stretchDown")).toEqual(["⌃⌥⇧↓"]);
    expect(label("win", "toolWindow.stretchRight")).toEqual(["Ctrl+Shift+→"]);
    expect(label("linux", "toolWindow.stretchUp")).toEqual(["Ctrl+Shift+↑"]);
    // 10-ui 「DB」 의 SQL 콘솔(M3-15)
    expect(label("mac", "db.console.open")).toEqual(["⇧⌘L"]);
    expect(label("win", "db.console.open")).toEqual(["Ctrl+Shift+F10"]);
    expect(label("mac", "db.console.run")).toEqual(["⌘↩"]);
    expect(label("linux", "db.console.run")).toEqual(["Ctrl+Enter"]);
    expect(label("mac", "db.console.cancel")).toEqual(["⌘F2"]);
    expect(label("win", "db.console.cancel")).toEqual(["Ctrl+F2"]);
    // 10-ui 「DB」 의 그리드 행 — 테이블 데이터(M3-17)
    expect(label("mac", "db.tableData.submit")).toEqual(["⌘↩"]);
    expect(label("win", "db.tableData.submit")).toEqual(["Ctrl+Enter"]);
    expect(label("mac", "db.tableData.revert")).toEqual(["⌥⌘Z"]);
    expect(label("linux", "db.tableData.revert")).toEqual(["Ctrl+Alt+Z"]);
    expect(label("mac", "db.tableData.addRow")).toEqual(["⌘N"]);
    expect(label("win", "db.tableData.addRow")).toEqual(["Alt+Insert"]);
    expect(label("mac", "db.tableData.deleteRow")).toEqual(["⌘⌫"]);
    expect(label("linux", "db.tableData.deleteRow")).toEqual(["Ctrl+Y"]);
  });
});

function view(keymap: unknown, error: ConfigView["error"] = null): ConfigView {
  return {
    snapshot: {
      files: keymap === undefined ? [] : [{ id: "keymap", value: keymap as never, layers: [] }],
      notices: [],
    },
    error,
  };
}

describe("resolveKeymap", () => {
  const base = defaults("mac");
  const find = (bindings: readonly { action: string; key: string }[]) =>
    bindings.filter((b) => b.action === "action.find").map((b) => b.key);

  it("is the default keymap without a user file", () => {
    const none = { bindings: base, warnings: [], good: { value: null } };
    expect(resolveKeymap(base, view(undefined), undefined, "mac")).toEqual(none);
    expect(resolveKeymap(base, undefined, undefined, "mac")).toEqual({
      bindings: base,
      warnings: [],
    });
  });

  it("applies the user's keymap.toml", () => {
    const resolved = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+k" }] }),
      undefined,
      "mac",
    );
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.error).toBeUndefined();
  });

  it("keeps the previous keymap and reports a malformed user file", () => {
    const previous = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+k" }] }),
      undefined,
      "mac",
    ).bindings;
    const resolved = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+nope" }] }),
      { base, bindings: previous },
      "mac",
    );
    expect(resolved.bindings).toBe(previous);
    expect(resolved.error).toContain("cmd+nope");
  });

  it("reports a TOML error in keymap.toml while using the snapshot the config layer kept", () => {
    const resolved = resolveKeymap(
      base,
      view(
        { bindings: [{ action: "action.find", key: "cmd+k" }] },
        { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" },
      ),
      undefined,
      "mac",
    );
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.error).toBe("expected `]`");
  });

  it("reports the TOML error, not the older snapshot's format error, when both are there", () => {
    const resolved = resolveKeymap(
      base,
      view(
        { bindings: [{ action: "action.find", key: "cmd+nope" }] },
        { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" },
      ),
      { base, bindings: base },
      "mac",
    );
    expect(resolved).toEqual({ bindings: base, error: "expected `]`", warnings: [] });
  });

  it("re-merges the last good user keymap when the default keymap changes under a malformed file", () => {
    const value = { bindings: [{ action: "action.find", key: "cmd+k" }] };
    const previous = resolveKeymap(base, view(value), undefined, "mac").bindings;
    // 확장 레지스트리에서 온 새 기본 키맵: 설정 열기를 ⌘J 로 둔다.
    const registryBase = readDefaultKeymap(
      parseKeymapToml(DEFAULT_KEYMAP_TOML.replace('mac = "cmd+,"', 'mac = "cmd+j"')),
      "mac",
    );
    expect(registryBase.find((b) => b.action === "settings.open")?.key).toBe("cmd+j");
    const malformed = view({ bindings: [{ action: "action.find", key: "cmd+nope" }] });
    const resolved = resolveKeymap(registryBase, malformed, { base, bindings: previous }, "mac", {
      value,
    });
    expect(find(resolved.bindings)).toEqual(["cmd+k"]);
    expect(resolved.bindings).toEqual(
      resolveKeymap(registryBase, view(value), undefined, "mac").bindings,
    );
    expect(resolved.error).toContain("cmd+nope");
    // 마지막 성공값도 없으면 새 기본 키맵 그대로다.
    expect(
      resolveKeymap(registryBase, malformed, { base, bindings: previous }, "mac").bindings,
    ).toBe(registryBase);
  });

  it("falls back to the app-wide last good keymap in a window that has none yet", () => {
    const lastGood = { value: { bindings: [{ action: "action.find", key: "cmd+k" }] } };
    const malformed = resolveKeymap(
      base,
      view({ bindings: [{ action: "action.find", key: "cmd+nope" }] }),
      undefined,
      "mac",
      lastGood,
    );
    expect(find(malformed.bindings)).toEqual(["cmd+k"]);
    expect(malformed.good).toBeUndefined();
    // 띄울 때부터 TOML 이 틀렸으면 설정 계층의 스냅숏에는 keymap 이 없다.
    const broken = resolveKeymap(
      base,
      view(undefined, { message: "expected `]`", path: "/cfg/pitwall/keymap.toml" }),
      undefined,
      "mac",
      lastGood,
    );
    expect(find(broken.bindings)).toEqual(["cmd+k"]);
  });

  it("keeps the last good keymap before the config arrives, and does not hand back a value", () => {
    const lastGood = { value: { bindings: [{ action: "action.find", key: "cmd+k" }] } };
    const early = resolveKeymap(base, undefined, undefined, "mac", lastGood);
    expect(find(early.bindings)).toEqual(["cmd+k"]);
    expect(early.good).toBeUndefined();
  });

  it("hands back what it read so it can become the last good keymap", () => {
    const value = { bindings: [{ action: "action.find", key: "cmd+k" }] };
    expect(resolveKeymap(base, view(value), undefined, "mac").good).toEqual({ value });
  });

  it("does not blame the keymap for another config file's error", () => {
    const resolved = resolveKeymap(
      base,
      view(undefined, { message: "bad", path: "/cfg/pitwall/settings.toml" }),
      undefined,
      "mac",
    );
    expect(resolved.error).toBeUndefined();
  });
});

describe("sharesKey", () => {
  it("finds actions bound to the same key in any context", () => {
    const bindings = defaults("mac");
    // 터미널의 ⌘W(`terminal.closeTab`, when terminalFocus)와 메뉴 「닫기」(`window.close`).
    expect(sharesKey(bindings, "terminal.closeTab", "window.close")).toBe(true);
    expect(sharesKey(bindings, "terminal.newTab", "window.close")).toBe(false);
  });
});
