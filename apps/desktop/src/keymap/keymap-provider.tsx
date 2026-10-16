// 창 하나의 키맵: 액션 레지스트리, 병합된 바인딩, 키 이벤트 → 엔진, 네이티브 메뉴 동기화, 액션 찾기 팝업.

import {
  type Action,
  ActionRegistry,
  type Binding,
  formatSequence,
  KeymapEngine,
  type LayoutMap,
  menuAccelerators,
  parseKeymapToml,
  type Platform,
  readDefaultKeymap,
  reservedReason,
} from "@pitwall/keymap";
import type { UiLogger } from "@pitwall/logger";
import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useRpcQuery } from "../rpc/host";
import { contextLayers } from "./context";
import { DEFAULT_KEYMAP_TOML } from "./default-keymap";
import { FindActionDialog } from "./find-action";
import type { KeymapNative } from "./native";
import { keymapPlatform } from "./platform";
import { resolveKeymap } from "./resolve";

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
  /** 액션의 단축키 라벨(병합 순서). */
  shortcuts(action: string): ShortcutLabel[];
  openFindAction(): void;
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
  log?: Pick<UiLogger, "warn">;
  children: ReactNode;
}) {
  const [registry] = useState(() => new ActionRegistry());
  const platform = useMemo(() => keymapPlatform(navigator.userAgent), []);
  const base = useMemo(
    () => readDefaultKeymap(parseKeymapToml(DEFAULT_KEYMAP_TOML), platform),
    [platform],
  );
  const config = useRpcQuery("config.get", {});
  const previous = useRef<readonly Binding[]>(undefined);
  const resolved = useMemo(
    () => resolveKeymap(base, config.data, previous.current, platform),
    [base, config.data, platform],
  );
  useEffect(() => {
    previous.current = resolved.bindings;
  }, [resolved.bindings]);
  useEffect(() => {
    if (!resolved.error) return;
    console.warn("[keymap] 사용자 키맵을 읽지 못해 이전 키맵을 쓴다", resolved.error);
    log?.warn(`keymap.toml ignored, keeping the previous keymap: ${resolved.error}`, "keymap");
  }, [resolved.error, log]);

  const [findOpen, setFindOpen] = useState(false);
  const popupOpen = useRef(false);
  popupOpen.current = findOpen;

  const [engine] = useState(
    () =>
      new KeymapEngine({
        run: (action) => registry.run(action),
        // 팝업이 열려 있는 동안 단축키는 팝업이 받는다 — 뒤의 창에서 액션이 돌지 않게.
        isEnabled: (action) => !popupOpen.current && registry.isEnabled(action),
        context: () =>
          contextLayers(document.activeElement, { popupOpen: popupOpen.current, platform }),
      }),
  );
  useEffect(() => engine.setBindings(resolved.bindings), [engine, resolved.bindings]);

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

  // 메뉴 accelerator 는 병합 결과에서 만든다(packages/keymap keymap-file.ts 머리 주석). 어떤 액션이 메뉴에 있는지는
  // 네이티브만 알므로, 아는 액션 전부를 보내고 메뉴에 없는 것은 네이티브가 버린다. 키가 모두 지워진 액션도 `null` 로
  // 보내야 메뉴의 옛 단축키가 지워진다 — 그래서 바인딩이 아니라 기본 키맵 · 레지스트리의 액션 목록에서 고른다.
  const version = useRegistryVersion(registry);
  useEffect(() => {
    const ids = [...new Set([...base.map((b) => b.action), ...registry.list().map((a) => a.id)])];
    native
      .syncMenu(menuAccelerators(resolved.bindings, ids))
      .catch((e: unknown) => console.warn("[keymap] menu sync failed", e));
  }, [native, base, registry, resolved.bindings, version]);

  useEffect(
    () =>
      native.onMenuAction((action) => {
        if (!registry.run(action)) console.warn("[keymap] menu action not available here", action);
      }),
    [native, registry],
  );

  const layout = useLayoutMap();
  const value = useMemo<KeymapContextValue>(
    () => ({
      registry,
      bindings: resolved.bindings,
      platform,
      error: resolved.error,
      shortcuts: (action) =>
        resolved.bindings
          .filter((b) => b.action === action)
          .map((b) => ({
            text: formatSequence(b.sequence, platform, layout),
            reserved: reservedReason(b.sequence, platform),
          })),
      openFindAction: () => setFindOpen(true),
    }),
    [registry, resolved, platform, layout],
  );

  return (
    <KeymapContext value={value}>
      {children}
      <FindActionDialog open={findOpen} onOpenChange={setFindOpen} />
    </KeymapContext>
  );
}

/** 키맵 파일 오류를 창 아래에 띄운다(10-ui §4.5 「오류는 알림으로」 — 알림 기능이 생기기 전의 최소 표시). */
export function KeymapErrorBar() {
  const { error } = useKeymap();
  if (!error) return null;
  return (
    <div
      role="alert"
      className="flex-none truncate border-t border-ui-accent bg-ui-panel px-2 py-0.5 text-[11px] text-ui-foreground"
      title={error}
    >
      키맵 파일 오류 — 이전 키맵을 쓰는 중: {error}
    </div>
  );
}
