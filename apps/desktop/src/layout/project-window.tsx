// 프로젝트 창(10-ui §2.4 S4)의 틀: 툴 윈도우 바 · 도킹 영역(툴 윈도우 · 에디터 영역) · 상태바. 내용은 자리만 잡는다.

import "dockview-react/dist/styles/dockview.css";

import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewPanelProps,
  themeLight,
} from "dockview-react";
import {
  Bell,
  Bot,
  Bug,
  Database,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  ListTree,
  type LucideIcon,
  Play,
  Search,
  Server,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { useRpcQuery } from "../rpc/host";
import { ThemeSwitcher, useTheme } from "../theme/theme-switcher";
import {
  type Dock,
  type LayoutStorage,
  type PanelComponent,
  PANELS,
  persistLayout,
  restoreLayout,
  showPanel,
} from "./project-layout";

const ICONS: Record<string, LucideIcon> = {
  project: FolderTree,
  commit: GitCommitHorizontal,
  database: Database,
  search: Search,
  structure: ListTree,
  run: Play,
  debug: Bug,
  problems: TriangleAlert,
  terminal: SquareTerminal,
  git: GitBranch,
  notifications: Bell,
  agent: Bot,
};

function ToolWindowPlaceholder({ api }: IDockviewPanelProps) {
  return (
    <div className="flex h-full items-center justify-center bg-ui-panel text-ui-muted-foreground">
      {api.title} — 툴 윈도우 자리
    </div>
  );
}

function EditorAreaPlaceholder() {
  return (
    <div className="flex h-full items-center justify-center bg-editor-background text-ui-muted-foreground">
      에디터 영역
    </div>
  );
}

// 저장본의 contentComponent 가 이 키를 가리킨다.
const COMPONENTS: Record<PanelComponent, (props: IDockviewPanelProps) => ReactNode> = {
  toolWindow: ToolWindowPlaceholder,
  editorArea: EditorAreaPlaceholder,
};

function ToolWindowBar({
  dock,
  api,
}: {
  dock: Exclude<Dock, "bottom">;
  api: DockviewApi | undefined;
}) {
  return (
    <nav
      aria-label={dock === "left" ? "왼쪽 툴 윈도우" : "오른쪽 툴 윈도우"}
      className={`flex w-8 flex-none flex-col items-center gap-1 border-ui-border bg-ui-toolbar py-1 ${dock === "left" ? "border-r" : "border-l"}`}
    >
      {PANELS.filter((p) => p.home === dock).map((panel) => {
        const Icon = ICONS[panel.id] ?? FolderTree;
        return (
          <button
            key={panel.id}
            type="button"
            title={panel.title}
            aria-label={panel.title}
            disabled={!api}
            onClick={() => api && showPanel(api, panel.id)}
            className="flex size-6 items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground"
          >
            <Icon size={16} aria-hidden />
          </button>
        );
      })}
    </nav>
  );
}

function StatusBar() {
  const info = useRpcQuery("host.info", null);
  const appearance = useTheme((s) => s.appearance);
  return (
    <footer className="flex h-[22px] flex-none items-center gap-3 border-t border-ui-border bg-ui-statusbar px-2 text-[11px] text-ui-statusbar-foreground">
      <span data-testid="status-host" className="flex items-center gap-1">
        <Server size={11} aria-hidden />
        {info.data
          ? `${info.data.hostId} · v${info.data.version}`
          : info.isError
            ? "연결 실패"
            : "…"}
      </span>
      <span className="flex-1" />
      <span data-testid="status-appearance">{appearance}</span>
      <ThemeSwitcher />
    </footer>
  );
}

/** 저장본으로 열고 바뀔 때마다 저장하는 도킹 영역. 열리면 `onApi` 로 api 를 넘긴다(툴 윈도우 바가 쓴다). */
export function ProjectDock({
  storage,
  onApi,
}: {
  storage: LayoutStorage;
  onApi(api: DockviewApi): void;
}) {
  const [api, setApi] = useState<DockviewApi>();

  const onReady = ({ api }: DockviewReadyEvent) => {
    console.info("[layout]", restoreLayout(api, storage));
    setApi(api);
    onApi(api);
  };

  // 저장 구독은 onReady 가 아니라 effect 에 건다. StrictMode 는 effect 를 해제했다 다시 붙이는데 onReady 는 다시 부르지
  // 않으므로, onReady 에서 걸고 effect 정리에서 풀면 첫 렌더 뒤로 저장이 조용히 멈춘다.
  useEffect(() => {
    if (!api) return;
    const persisted = persistLayout(api, storage);
    return () => persisted.dispose();
  }, [api, storage]);

  return (
    <DockviewReact
      className="min-w-0 flex-1"
      theme={themeLight}
      components={COMPONENTS}
      onReady={onReady}
    />
  );
}

export function ProjectWindow() {
  const [api, setApi] = useState<DockviewApi>();
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1">
        <ToolWindowBar dock="left" api={api} />
        <ProjectDock storage={window.localStorage} onApi={setApi} />
        <ToolWindowBar dock="right" api={api} />
      </div>
      <StatusBar />
    </div>
  );
}
