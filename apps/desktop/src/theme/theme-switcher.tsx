import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { Monitor, Moon, Sun } from "lucide-react";
import { createContext, type ReactNode, useContext } from "react";
import { useStore } from "zustand";

import { useAppStrings } from "../i18n/app-strings";
import type { ThemePreference, ThemeState, ThemeStore } from "./theme-store";

const ThemeStoreContext = createContext<ThemeStore | undefined>(undefined);

export function ThemeStoreProvider({
  store,
  children,
}: {
  store: ThemeStore;
  children: ReactNode;
}) {
  return <ThemeStoreContext value={store}>{children}</ThemeStoreContext>;
}

export function useTheme<T>(selector: (state: ThemeState) => T): T {
  const store = useContext(ThemeStoreContext);
  if (!store) throw new Error("ThemeStoreProvider 밖에서 useTheme 을 불렀다");
  return useStore(store, selector);
}

const OPTIONS: { value: ThemePreference; icon: typeof Sun }[] = [
  { value: "system", icon: Monitor },
  { value: "light", icon: Sun },
  { value: "dark", icon: Moon },
];

export function ThemeSwitcher() {
  const preference = useTheme((s) => s.preference);
  const setPreference = useTheme((s) => s.setPreference);
  const T = useAppStrings().theme;
  return (
    <ToggleGroup.Root
      type="single"
      aria-label={T.label}
      value={preference}
      // 이미 눌린 항목을 다시 누르면 Radix 는 빈 값을 준다. 선택 해제는 없으므로 무시한다.
      onValueChange={(value) => {
        if (value) setPreference(value as ThemePreference);
      }}
      className="flex items-center"
    >
      {OPTIONS.map(({ value, icon: Icon }) => (
        <ToggleGroup.Item
          key={value}
          value={value}
          aria-label={T[value]}
          title={T[value]}
          data-testid={`theme-${value}`}
          className="flex h-[18px] w-[22px] items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover focus-visible:outline-1 focus-visible:outline-ui-accent data-[state=on]:bg-ui-active data-[state=on]:text-ui-foreground"
        >
          <Icon size={12} aria-hidden />
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}
