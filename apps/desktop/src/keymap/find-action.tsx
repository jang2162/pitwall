// 액션 찾기(⌘⇧A, 10-ui §1 원칙 2). 상단 검색 팝업(ui/search-popup.tsx)에 등록된 액션을 늘어놓는다.

import { Command } from "cmdk";

import { useAppStrings } from "../i18n/app-strings";
import { useI18n } from "../i18n/i18n-provider";
import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";
import { useKeymap, useRegistryVersion } from "./keymap-provider";

export function FindActionDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
}) {
  const { registry, shortcuts, runnable } = useKeymap();
  const T = useAppStrings().findAction;
  const { locales } = useI18n();
  useRegistryVersion(registry);
  const actions = open
    ? registry
        .list()
        .filter((a) => runnable(a.id) && a.id !== "action.find")
        .sort((a, b) => a.label.localeCompare(b.label, locales[0]))
    : [];

  return (
    <SearchPopup
      open={open}
      onOpenChange={onOpenChange}
      label={T.label}
      placeholder={T.placeholder}
      empty={T.empty}
    >
      {actions.map((action) => {
        const keys = shortcuts(action.id);
        return (
          <Command.Item
            key={action.id}
            value={action.id}
            keywords={[action.label, ...(action.keywords ?? [])]}
            onSelect={() => {
              onOpenChange(false);
              // 팝업이 닫히며 포커스를 이전 자리로 돌려놓은 뒤에 실행한다 — 액션이 옮긴 포커스를 덮지 않게.
              setTimeout(() => registry.run(action.id), 0);
            }}
            className={SEARCH_ITEM_CLASS}
          >
            <span className="flex-1 truncate">{action.label}</span>
            {keys.map((k) => (
              <kbd
                key={k.text}
                title={k.reserved ? T.reserved(k.reserved) : undefined}
                className={`font-sans text-[11px] text-ui-muted-foreground ${k.reserved ? "line-through" : ""}`}
              >
                {k.text}
              </kbd>
            ))}
          </Command.Item>
        );
      })}
    </SearchPopup>
  );
}
