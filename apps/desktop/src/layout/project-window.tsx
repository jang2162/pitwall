// 프로젝트 창(10-ui §2.4 S4)의 틀: 툴 윈도우 바 · 도킹 영역(툴 윈도우 · 에디터 영역) · 상태바. 내용은 자리만 잡는다.

import "dockview-react/dist/styles/dockview.css";

import {
  type DockviewApi,
  DockviewDefaultTab,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewPanelHeaderProps,
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
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { useAppStrings } from "../i18n/app-strings";
import { toolWindowActions } from "../keymap/actions";
import { useRegisterActions } from "../keymap/keymap-provider";
import { activeWorktree } from "../open-with/active-worktree";
import { type OpenWithTarget, useOpenWith } from "../open-with/open-with";
import { projectIdFromQuery } from "../project-open";
import { useRpcQuery } from "../rpc/host";
import { ThemeSwitcher, useTheme } from "../theme/theme-switcher";
import {
  type Dock,
  EDITOR_TAB,
  keepEditor,
  type LayoutStorage,
  type PanelComponent,
  PANELS,
  persistLayout,
  restoreLayout,
  showPanel,
  trackHiddenPanels,
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

// `data-tool-window` 는 키맵 컨텍스트의 툴 윈도우 층이다(src/keymap/context.ts).
function ToolWindowPlaceholder({ api }: IDockviewPanelProps) {
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full items-center justify-center bg-ui-panel text-ui-muted-foreground"
    >
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

// hideClose 는 × 버튼과 가운데 클릭 닫기를 함께 끈다. 키보드 닫기는 막지 못한다 — keepEditor 가 받는다.
const TAB_COMPONENTS = {
  [EDITOR_TAB]: (props: IDockviewPanelHeaderProps) => <DockviewDefaultTab {...props} hideClose />,
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
    const editor = keepEditor(api);
    const hidden = trackHiddenPanels(api);
    return () => {
      hidden.dispose();
      editor.dispose();
      persisted.dispose();
    };
  }, [api, storage]);

  return (
    <DockviewReact
      className="min-w-0 flex-1"
      theme={themeLight}
      components={COMPONENTS}
      tabComponents={TAB_COMPONENTS}
      onReady={onReady}
    />
  );
}

/** 이 창의 프로젝트를 외부 도구로 연다(⌥F1 · ⌘⌥E, open-with/open-with.tsx). 실패 문장은 상태바 위 한 줄에 둔다. */
function ProjectOpenWith({ projectId }: { projectId: string }) {
  const T = useAppStrings().openWith;
  const project = useRpcQuery("project.get", { id: projectId });
  const [error, setError] = useState<string>();
  const target = useRef<OpenWithTarget>(undefined);
  target.current = project.data
    ? { projectId, name: project.data.name, worktree: activeWorktree() }
    : undefined;
  const { actions, popup } = useOpenWith({ target: () => target.current, onError: setError });
  useRegisterActions(actions);
  return (
    <>
      {popup}
      {error && (
        <div
          role="alert"
          className="flex flex-none items-center gap-2 border-t border-ui-accent px-2 py-0.5 text-[12px]"
        >
          <span className="min-w-0 flex-1 truncate" title={error}>
            {error}
          </span>
          <button
            type="button"
            onClick={() => setError(undefined)}
            className="flex-none text-ui-muted-foreground hover:text-ui-foreground"
          >
            {T.dismiss}
          </button>
        </div>
      )}
    </>
  );
}

export function ProjectWindow({
  projectId = projectIdFromQuery(window.location.search),
}: {
  /** 창 쿼리의 프로젝트 id(`?project=`). 없으면 외부 도구 액션이 없다. */
  projectId?: string;
}) {
  const [api, setApi] = useState<DockviewApi>();
  const apiRef = useRef<DockviewApi>(undefined);
  apiRef.current = api;
  const actions = useMemo(() => toolWindowActions(() => apiRef.current), []);
  useRegisterActions(actions);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1">
        <ToolWindowBar dock="left" api={api} />
        <ProjectDock storage={window.localStorage} onApi={setApi} />
        <ToolWindowBar dock="right" api={api} />
      </div>
      {projectId !== undefined && <ProjectOpenWith projectId={projectId} />}
      <StatusBar />
    </div>
  );
}
