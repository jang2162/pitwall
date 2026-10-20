import { RpcClient } from "@pitwall/rpc-client";
import { invoke } from "@tauri-apps/api/core";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../extensions/theme-default/themes/default.json";
import { App } from "./App";
import { FirstFrameMark, perfMarkRequested } from "./perf-mark";
import { connectHost } from "./tauri-transport";
import "./styles.css";
import {
  documentThemeApplier,
  localPreferenceStorage,
  mediaAppearance,
  tauriWindowTheme,
} from "./theme/adapters";
import { rememberActiveWorktree } from "./open-with/active-worktree";
import { applyRegistryTheme } from "./theme/registry-theme";
import { createThemeStore } from "./theme/theme-store";
import { ThemeStoreProvider } from "./theme/theme-switcher";
import { parseTheme } from "./theme/tokens";
import {
  describeOpen,
  projectIdFromQuery,
  projectOpenInbox,
  tauriProjectOpenSource,
  watchProjectOpens,
} from "./project-open";
import { installUiLog } from "./ui-log";
import { detectPlatform, installWebviewGuards } from "./webview-guards";
import { parseWindowKind } from "./window-kind";

const log = installUiLog(window);
const windowKind = parseWindowKind(window.location.search);
log.info(`window opened: ${windowKind}`, "main");
// 프로젝트 창이 받은 열기 요청(src-tauri/src/launch.rs · windows.rs). 창의 활성 워크트리를 바꾸고(파일 트리 · 탭이 없을 때의
// 외부 도구가 쓴다) 우편함에 넣는다 — 에디터가 마운트되면 꺼내 탭을 연다. 탭의 파일 읽기는 이 값이 아니라 요청에 실린
// `worktree` 를 쓴다(src/editor/editor-workspace.tsx 머리 주석).
if (windowKind === "project") {
  log.info(`project window: ${projectIdFromQuery(window.location.search) ?? "(none)"}`, "project");
  watchProjectOpens(tauriProjectOpenSource, (request) => {
    log.info(`open request: ${describeOpen(request) ?? "(focus only)"}`, "project");
    rememberActiveWorktree(request);
    projectOpenInbox.push(request);
  });
}
const platform = detectPlatform(navigator.userAgent);
installWebviewGuards(window, platform);
document.documentElement.dataset.platform = platform.mac ? "mac" : "other";

// 렌더 전에 만든다 — 첫 그림부터 저장된 테마의 색으로 그린다. 번들의 기본 테마로 먼저 칠하고 확장 레지스트리의 것으로
// 바꾼다(theme/registry-theme.ts).
const themeStore = createThemeStore({
  system: mediaAppearance(window),
  storage: localPreferenceStorage(window),
  native: tauriWindowTheme(),
  theme: parseTheme(defaultTheme),
  apply: documentThemeApplier(document.documentElement),
});
void applyRegistryTheme(
  themeStore,
  async () => {
    // src-tauri/src/main.rs 의 LOCAL_HOST_ID. 레지스트리는 이 기기의 Host 가 갖는다.
    const conn = await connectHost({ hostId: "local" });
    const client = new RpcClient(conn.transport);
    return {
      client,
      close() {
        client.close();
        void conn.close();
      },
    };
  },
  log,
);

// src-tauri/src/perf.rs 의 perf_first_frame · perf_hidden. 측정 모드가 아니면 그리지 않는다(perf-mark.tsx).
function markFirstFrame(mark: { hidden: boolean }) {
  invoke("perf_first_frame", mark).catch((e: unknown) =>
    log.warn(`perf mark failed: ${String(e)}`, "perf"),
  );
}

function markHidden() {
  invoke("perf_hidden").catch((e: unknown) => log.warn(`perf mark failed: ${String(e)}`, "perf"));
}

const root = document.getElementById("root");
if (!root) throw new Error("#root 가 index.html 에 없다");

createRoot(root).render(
  <StrictMode>
    <ThemeStoreProvider store={themeStore}>
      <App kind={windowKind} log={log} />
    </ThemeStoreProvider>
    {perfMarkRequested(window.location.search) && (
      <FirstFrameMark onFrame={markFirstFrame} onHidden={markHidden} />
    )}
  </StrictMode>,
);
