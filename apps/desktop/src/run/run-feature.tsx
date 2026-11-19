// 프로젝트 창의 실행 배선(17-m6-plan M6-06): 창의 실행 탭(run-tabs.ts)을 Host 에 붙이고, 탭 · 콘솔이 쓰는 지금 값(키맵 · 링크 ·
// 문자열)을 넣고, 상태바 `run` 항목(실행 중인 수 — 누르면 「실행」 툴 윈도우)을 둔다. 툴 윈도우를 열지 않아도 붙는다 — 에이전트가
// 띄운 실행의 탭(④)과 상태바 수 · 닫기 확인(⑥)이 툴 윈도우와 상관없이 맞아야 한다.

import { openUrl } from "@tauri-apps/plugin-opener";
import { Play } from "lucide-react";
import { useEffect } from "react";
import { useStore } from "zustand";

import { useEditorWorkspace } from "../editor/editor-workspace";
import { useKeymap } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useHost } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { useTheme } from "../theme/theme-switcher";
import { terminalTheme } from "../terminal/terminal-theme";
import { useRiskConfirm } from "../db/safety/risk-confirm";
import { RunChainNotices } from "./run-chain";
import { RunStatusPorts } from "./run-ports";
import { isLive, type RunTabs, windowRuns } from "./run-tabs";
import { sqlRunQuestion } from "./sql-run";
import { useRunStrings } from "./strings";

/** 상태바 항목: 이 프로젝트에서 도는 실행 수. 0 이면 그리지 않는다. */
export function RunStatusItem() {
  const T = useRunStrings().status;
  const { registry } = useKeymap();
  const tabs = useStore(windowRuns().store, (s) => s.tabs);
  const live = tabs.filter((t) => isLive(t.run));
  if (live.length === 0) return null;
  return (
    <span className="flex items-center gap-1">
      <button
        type="button"
        data-testid="status-run"
        title={T.hint(live.length)}
        onClick={() => void registry.run("toolWindow.run")}
        className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
      >
        <Play size={11} aria-hidden />
        {T.label(live.length)}
      </button>
      <RunStatusPorts runs={live.map((t) => t.run)} />
    </span>
  );
}

/** 이 창의 실행을 Host 에 붙이고 상태바 항목을 둔다. `StatusBarProvider` · 작업 공간 안에 둔다. */
export function RunFeature({
  projectId,
  tabs = windowRuns(),
}: {
  projectId: string;
  tabs?: RunTabs;
}) {
  const T = useRunStrings();
  const terminalStrings = useProjectStrings().terminal;
  const { client } = useHost();
  const { platform, takesKey } = useKeymap();
  const workspace = useEditorWorkspace();
  const theme = useTheme((s) => s.theme);
  // 다시 실행하는 사슬의 `sql` 단계 위험 확인(M6-21) — SQL 콘솔과 같은 다이얼로그.
  const risk = useRiskConfirm();
  const appearance = useTheme((s) => s.appearance);

  if (client) {
    tabs.setDeps({
      client,
      projectId,
      takesKey,
      opensLink: (e) => (platform === "mac" ? e.metaKey : e.ctrlKey),
      openFile: (target) => void workspace.open(target),
      openUrl: (url) =>
        void openUrl(url).catch((e: unknown) => console.warn("[run] link not opened", e)),
      exitedLine: T.exitedLine,
      droppedLine: T.droppedLine,
      failedLine: T.attachFailed,
      linkHint: terminalStrings.linkHint,
      log: (message) => console.info("[run]", message),
      askSql: async (data) => {
        const answer = await risk.confirm(sqlRunQuestion(data));
        return answer === undefined || answer === "busy" ? undefined : answer;
      },
    });
  }

  useEffect(
    () => (client ? tabs.connect(client, projectId) : undefined),
    [tabs, client, projectId],
  );

  useEffect(() => {
    const values = theme.variants[appearance];
    if (values) tabs.setTheme(terminalTheme(values));
  }, [tabs, theme, appearance]);

  useStatusBarItem({ id: "run", ...STATUS_SLOTS.run, component: RunStatusItem });
  return (
    <>
      {client && <RunChainNotices client={client} projectId={projectId} />}
      {risk.element}
    </>
  );
}
