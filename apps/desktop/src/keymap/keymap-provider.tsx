// 창 하나의 키맵: 액션 레지스트리, 병합된 바인딩, 키 이벤트 → 엔진, 네이티브 메뉴 동기화, Search Everywhere 팝업(액션 찾기는 그
// 액션 탭이다, src/search-everywhere/search-everywhere.tsx).

import {
  type Action,
  ActionRegistry,
  type Binding,
  formatSequence,
  KeymapEngine,
  type LayoutMap,
  menuAccelerators,
  type Platform,
  reachableBindings,
  reservedReason,
} from "@pitwall/keymap";
import type { UiLogger } from "@pitwall/logger";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useAppStrings } from "../i18n/app-strings";
import { useRpcQuery } from "../rpc/host";
import { terminalAdmits } from "../terminal/terminal-keys";
import { contextLayers } from "./context";
import { chooseDefaultKeymap, DEFAULT_KEYMAP_ID, registryKeymapProblem } from "./default-keymap";
import {
  availableMode,
  SearchEverywhere,
  type SearchMode,
  type SearchSource,
} from "../search-everywhere/search-everywhere";
import type { KeymapNative } from "./native";
import { keymapPlatform } from "./platform";
import { type LastGoodKeymap, type PreviousKeymap, resolveKeymap } from "./resolve";

/**
 * 창이 띄운 팝업(`useKeymapPopup`)이 열려 있어도 네이티브 메뉴에서 고르면 도는 액션. 다른 창을 열 뿐 팝업 뒤의 창 내용을 바꾸지
 * 않는 것만 둔다. 앱 종료 같은 OS 기본 메뉴 항목은 키맵 액션이 아니라 이 규칙과 무관하게 돈다. 「닫기」(`window.close`)는 키맵
 * 액션이라 팝업이 열린 동안에는 메뉴로 와도 돌지 않는다.
 */
export const MENU_ACTIONS_OVER_POPUPS: ReadonlySet<string> = new Set(["settings.open"]);

/** 키로 실행한 액션이 같은 눌림의 메뉴 accelerator 로 또 오는 것을 버리는 시간 창. */
export const MENU_DUPLICATE_MS = 300;

// 앱 전체(같은 origin 의 창들)가 함께 쓰는 마지막으로 읽힌 사용자 키맵(resolve.ts 머리 주석). 저장소가 막혀도 동작한다.
const LAST_GOOD_KEY = "pitwall.keymap.lastGood";

function readLastGood(): LastGoodKeymap | undefined {
  try {
    const raw = localStorage.getItem(LAST_GOOD_KEY);
    return raw === null ? undefined : (JSON.parse(raw) as LastGoodKeymap);
  } catch {
    return undefined;
  }
}

function writeLastGood(good: LastGoodKeymap) {
  try {
    localStorage.setItem(LAST_GOOD_KEY, JSON.stringify(good));
  } catch {
    // 못 남겨도 이 창은 자기 직전 키맵(`previous`)으로 버틴다.
  }
}

export interface ShortcutLabel {
  readonly text: string;
  /** OS 가 가져가 앱에 오지 않는 키면 그 이유(10-ui §4.4 「OS 예약 키」). */
  readonly reserved?: string;
}

export interface KeymapContextValue {
  readonly registry: ActionRegistry;
  readonly bindings: readonly Binding[];
  readonly platform: Platform;
  /** 사용자 키맵 파일의 오류. 있으면 `bindings` 는 마지막으로 읽힌 키맵이다. */
  readonly error?: string;
  /** 사용자 키맵 파일의 경고(그 OS 에 없는 키 등). */
  readonly warnings: readonly string[];
  /** 액션의 단축키 라벨(병합 순서). */
  shortcuts(action: string): ShortcutLabel[];
  /** 창이 띄운 팝업 상태로 보아 지금 실행할 수 있는가 — 키와 같은 기준(액션 찾기 목록이 쓴다). */
  runnable(action: string): boolean;
  /**
   * 이 keydown 을 창의 키맵이 가져가는가(실행하지 않고 본다). 키를 먼저 받는 요소(터미널)가 창 단축키에 양보할지 정한다 —
   * 참이면 그 요소는 키를 처리하지 않고 창으로 흘려보낸다(src/terminal/terminal-keys.ts).
   */
  takesKey(e: KeyboardEvent): boolean;
  openFindAction(): void;
  /** Search Everywhere 를 그 탭(모드)으로 연다. 그 창에 없는 탭이면 전체 탭이다. */
  openSearch(mode: SearchMode): void;
  /** Search Everywhere 에 결과 묶음(파일 · 프로젝트 · 최근 파일)을 더한다. 해제 함수를 돌려준다. */
  registerSearchSource(source: SearchSource): () => void;
  /** 팝업이 열렸다고 알린다(`useKeymapPopup`). 닫는 함수를 돌려준다. */
  enterPopup(): () => void;
}

const KeymapContext = createContext<KeymapContextValue | undefined>(undefined);

export function useKeymap(): KeymapContextValue {
  const value = useContext(KeymapContext);
  if (!value) throw new Error("KeymapProvider 밖에서 useKeymap 을 불렀다");
  return value;
}

/** 액션을 이 창의 레지스트리에 등록한다. `actions` 는 메모해서 넘긴다 — 바뀔 때마다 다시 등록한다. */
export function useRegisterActions(actions: readonly Action[]) {
  const { registry } = useKeymap();
  useEffect(() => {
    const offs = actions.map((a) => registry.register(a));
    return () => offs.forEach((off) => off());
  }, [registry, actions]);
}

/**
 * 창이 띄운 팝업(시트 · 대화상자 · 우클릭 메뉴)이 열려 있는 동안 키를 팝업 액션(`Action.popup`)에만 준다 — 액션 찾기와 같다.
 * 팝업을 그리는 컴포넌트가 자기 `open` 으로 부른다.
 */
export function useKeymapPopup(open: boolean) {
  const { enterPopup } = useKeymap();
  useEffect(() => (open ? enterPopup() : undefined), [open, enterPopup]);
}

/** Search Everywhere 에 결과 묶음을 더한다. `source` 는 메모해서 넘긴다. */
export function useSearchSource(source: SearchSource | undefined) {
  const { registerSearchSource } = useKeymap();
  useEffect(
    () => (source ? registerSearchSource(source) : undefined),
    [registerSearchSource, source],
  );
}

/** 레지스트리가 바뀌면 다시 그린다. */
export function useRegistryVersion(registry: ActionRegistry): number {
  return useSyncExternalStore(registry.subscribe, registry.getVersion);
}

/** WebKit 에는 `navigator.keyboard` 가 없다(packages/keymap keys.ts `LayoutMap`). 있으면 그 배열로 라벨을 그린다. */
function useLayoutMap(): LayoutMap | undefined {
  const [layout, setLayout] = useState<LayoutMap>();
  useEffect(() => {
    const keyboard = (navigator as { keyboard?: { getLayoutMap?: () => Promise<LayoutMap> } })
      .keyboard;
    keyboard
      ?.getLayoutMap?.()
      .then(setLayout)
      .catch(() => {});
  }, []);
  return layout;
}

export function KeymapProvider({
  native,
  log,
  children,
}: {
  native: KeymapNative;
  log?: Pick<UiLogger, "info" | "warn" | "debug">;
  children: ReactNode;
}) {
  const [registry] = useState(() => new ActionRegistry());
  const platform = useMemo(() => keymapPlatform(navigator.userAgent), []);
  // 기본 키맵은 확장 레지스트리에서 받고, 받기 전 · 실패하면 번들 것을 쓴다(default-keymap.ts).
  // 레지스트리는 앱이 시작할 때 한 번 읽으므로 다시 부를 일이 없다. 창 포커스 · 재연결 때마다 다시 부르지 않고, 실패도
  // 다시 부르지 않는다(실패는 번들 키맵으로 이미 덮여 있다).
  const registryKeymap = useRpcQuery(
    "keymap.get",
    { id: DEFAULT_KEYMAP_ID },
    { staleTime: Infinity, retry: false },
  );
  const defaultKeymap = useMemo(
    () => chooseDefaultKeymap(registryKeymap.data, platform),
    [registryKeymap.data, platform],
  );
  const base = defaultKeymap.bindings;
  const registryProblem = registryKeymapProblem(registryKeymap, defaultKeymap);
  const registryExtension =
    defaultKeymap.source.kind === "registry" ? defaultKeymap.source.extension : undefined;
  useEffect(() => {
    if (registryProblem) {
      log?.warn(
        `keymap ${DEFAULT_KEYMAP_ID} not loaded from the extension registry, using the bundled copy: ${registryProblem}`,
        "keymap",
      );
    } else if (registryExtension) {
      log?.info(
        `keymap ${DEFAULT_KEYMAP_ID} applied from extension ${registryExtension}`,
        "keymap",
      );
    }
  }, [registryProblem, registryExtension, log]);
  const config = useRpcQuery("config.get", {});
  const previous = useRef<PreviousKeymap>(undefined);
  const resolved = useMemo(
    () => resolveKeymap(base, config.data, previous.current, platform, readLastGood()),
    [base, config.data, platform],
  );
  useEffect(() => {
    previous.current = { base, bindings: resolved.bindings };
    if (resolved.good) writeLastGood(resolved.good);
  }, [base, resolved]);
  useEffect(() => {
    if (!resolved.error) return;
    console.warn("[keymap] 사용자 키맵을 읽지 못해 이전 키맵을 쓴다", resolved.error);
    log?.warn(`keymap.toml ignored, keeping the previous keymap: ${resolved.error}`, "keymap");
  }, [resolved.error, log]);
  const warnings = resolved.warnings.join("; ");
  useEffect(() => {
    if (!warnings) return;
    console.warn("[keymap]", warnings);
    log?.warn(`keymap.toml: ${warnings}`, "keymap");
  }, [warnings, log]);

  const [search, setSearch] = useState<SearchMode>();
  const [sources, setSources] = useState<readonly SearchSource[]>([]);
  const registerSearchSource = useCallback((source: SearchSource) => {
    setSources((list) => [...list.filter((s) => s.id !== source.id), source]);
    return () => setSources((list) => list.filter((s) => s !== source));
  }, []);
  const sourcesRef = useRef(sources);
  sourcesRef.current = sources;
  const openSearch = useCallback(
    (mode: SearchMode) => setSearch(availableMode(mode, sourcesRef.current)),
    [],
  );
  // Search Everywhere 말고 창이 띄운 팝업의 수(`useKeymapPopup`). 렌더가 아니라 키 처리에서만 읽으므로 ref 다.
  const otherPopups = useRef(0);
  const searchOpenRef = useRef(false);
  searchOpenRef.current = search !== undefined;
  const isPopupOpen = useCallback(() => searchOpenRef.current || otherPopups.current > 0, []);
  // 창이 띄운 팝업이 열려 있으면 팝업 액션만, 아니면 팝업 아닌 액션만 돈다. 키 · 메뉴 · 액션 찾기 목록이 같은 기준을 쓴다.
  const windowGate = useCallback(
    (action: string) => otherPopups.current > 0 === registry.isPopup(action),
    [registry],
  );
  const enterPopup = useCallback(() => {
    otherPopups.current++;
    let left = false;
    return () => {
      if (left) return;
      left = true;
      otherPopups.current--;
    };
  }, []);

  // 키로 실행한 마지막 액션. 같은 눌림이 메뉴 accelerator 로도 와서 두 번 실행되는 것을 막는다(아래 onMenuAction).
  const lastKeyRun = useRef<{ action: string; at: number }>(undefined);
  const [engine] = useState(
    () =>
      new KeymapEngine({
        run: (action) => {
          const ran = registry.run(action);
          if (ran) {
            lastKeyRun.current = { action, at: performance.now() };
            log?.debug(`ran ${action} from a key`, "keymap");
          }
          return ran;
        },
        allowsRepeat: (action) => registry.allowsRepeat(action),
        // 팝업이 열려 있는 동안 단축키는 팝업이 받는다 — 뒤의 창에서 액션이 돌지 않게. 팝업 액션은 팝업이 열려 있을 때만 돈다.
        isEnabled: (action) =>
          isPopupOpen() === registry.isPopup(action) && registry.isEnabled(action),
        context: () =>
          contextLayers(document.activeElement, { popupOpen: isPopupOpen(), platform }),
        admits: terminalAdmits,
      }),
  );
  useEffect(() => engine.setBindings(resolved.bindings), [engine, resolved.bindings]);
  const bindingsRef = useRef(resolved.bindings);
  bindingsRef.current = resolved.bindings;

  useEffect(() => {
    const onDown = (e: KeyboardEvent) => {
      if (engine.keydown(e).handled) e.preventDefault();
    };
    const onUp = (e: KeyboardEvent) => {
      if (engine.keyup(e).handled) e.preventDefault();
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  }, [engine]);

  // 메뉴 accelerator 는 병합 결과에서 만든다(packages/keymap keymap-file.ts 머리 주석). 가려진 키는 빼고 보낸다
  // (`reachableBindings`). 어떤 액션이 메뉴에 있는지는
  // 네이티브만 알므로, 아는 액션 전부를 보내고 메뉴에 없는 것은 네이티브가 버린다. 키가 모두 지워진 액션도 `null` 로
  // 보내야 메뉴의 옛 단축키가 지워진다 — 그래서 바인딩이 아니라 기본 키맵 · 레지스트리의 액션 목록에서 고른다.
  const version = useRegistryVersion(registry);
  useEffect(() => {
    const ids = [...new Set([...base.map((b) => b.action), ...registry.list().map((a) => a.id)])];
    native
      .syncMenu(menuAccelerators(resolved.bindings, ids, platform))
      .catch((e: unknown) => console.warn("[keymap] menu sync failed", e));
  }, [native, base, registry, resolved.bindings, version, platform]);

  // ⚠ 한 번의 키 눌림이 웹뷰 keydown 과 메뉴 accelerator 둘 다로 올 수 있다. macOS(WKWebView)는 웹이 처리(preventDefault)한
  // key equivalent 를 메뉴로 보내지 않는다고 보지만 실측하지 않았고, Windows · Linux 는 순서 · 배타성을 모른다. 그래서
  // 키로 실행한 직후(`MENU_DUPLICATE_MS` 안) 같은 액션이 메뉴로 오면 버린다. 다른 액션이어도 그 키를 함께 쓰면(`sharesKey` —
  // 터미널 포커스의 ⌘W 는 `terminal.closeTab`, 메뉴 「닫기」 는 `window.close`) 같은 눌림으로 보고 버린다. 이 가드가 있어도 **메뉴에 넣는 액션은 두 번
  // 실행돼도 결과가 같아야 한다**(열기 · 보이기) — 가드의 시간 창 밖에서 늦게 온 메뉴 이벤트는 걸러지지 않는다. 토글
  // 액션을 메뉴에 넣으려면 먼저 그 경로가 정말 한 번만 오는지 실측하거나 더 단단한 중복 제거를 둔다(src-tauri/src/menu.rs).
  useEffect(
    () =>
      native.onMenuAction((action) => {
        const last = lastKeyRun.current;
        if (
          last &&
          performance.now() - last.at < MENU_DUPLICATE_MS &&
          (last.action === action || sharesKey(bindingsRef.current, last.action, action))
        ) {
          log?.debug(
            `dropped ${action} from the menu: the key already ran ${last.action}`,
            "keymap",
          );
          return;
        }
        // Search Everywhere(액션 찾기)가 열린 채 메뉴에서 고르면 팝업을 닫고 실행한다. 메뉴 이벤트로는 마우스로 고른 것인지 키인지 알 수 없는데,
        // 마우스로 메뉴를 고른 것을 조용히 무시하면 고장으로 보인다. 창이 띄운 팝업(`useKeymapPopup`)은 그 팝업이 닫는다.
        if (!windowGate(action) && !MENU_ACTIONS_OVER_POPUPS.has(action)) {
          log?.debug(`dropped ${action} from the menu: a popup is open`, "keymap");
          return;
        }
        const run = () => {
          if (registry.run(action)) log?.debug(`ran ${action} from the menu`, "keymap");
          else console.warn("[keymap] menu action not available here", action);
        };
        if (searchOpenRef.current) {
          setSearch(undefined);
          setTimeout(run, 0);
        } else run();
      }),
    [native, registry, log, windowGate],
  );

  const layout = useLayoutMap();
  const value = useMemo<KeymapContextValue>(
    () => ({
      registry,
      bindings: resolved.bindings,
      platform,
      error: resolved.error,
      warnings: resolved.warnings,
      shortcuts: (action) =>
        reachableBindings(resolved.bindings)
          .filter((b) => b.action === action)
          .map((b) => ({
            text: formatSequence(b.sequence, platform, layout),
            reserved: reservedReason(b.sequence, platform),
          })),
      openFindAction: () => openSearch("actions"),
      openSearch,
      registerSearchSource,
      enterPopup,
      runnable: (action) => windowGate(action) && registry.isEnabled(action),
      takesKey: (e) => engine.peek(e),
    }),
    [
      registry,
      resolved,
      platform,
      layout,
      enterPopup,
      windowGate,
      engine,
      openSearch,
      registerSearchSource,
    ],
  );

  return (
    <KeymapContext value={value}>
      {children}
      <SearchEverywhere mode={search} onModeChange={setSearch} sources={sources} />
    </KeymapContext>
  );
}

/** 두 액션이 같은 키를 하나라도 함께 쓰는가(`when` 은 보지 않는다). */
export function sharesKey(bindings: readonly Binding[], a: string, b: string): boolean {
  const keysOf = (action: string) =>
    new Set(bindings.filter((x) => x.action === action).map((x) => JSON.stringify(x.sequence)));
  const ofA = keysOf(a);
  return [...keysOf(b)].some((k) => ofA.has(k));
}

/** 키맵 파일 오류 · 경고를 창 아래에 띄운다(10-ui §4.5 「오류는 알림으로」 — 알림 기능이 생기기 전의 최소 표시). */
export function KeymapErrorBar() {
  const { error, warnings } = useKeymap();
  const T = useAppStrings().keymap;
  const text = error
    ? T.error(error)
    : warnings.length > 0
      ? T.warning(warnings.join("; "))
      : undefined;
  if (!text) return null;
  return (
    <div
      role="alert"
      className="flex-none truncate border-t border-ui-accent bg-ui-panel px-2 py-0.5 text-[11px] text-ui-foreground"
      title={text}
    >
      {text}
    </div>
  );
}
