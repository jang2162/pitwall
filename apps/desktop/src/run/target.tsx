// 실행 · 터미널의 대상 체크아웃(18-m7-plan M7-14 ① — 10-ui.md 2.9 「실행 · 터미널의 대상」). 실행 선택기와 「터미널 열기」 가 함께
// 쓰는 고르기다: 기본은 활성 체크아웃을 따라가고, 사람이 고르면 그 체크아웃이다. 고를 수 있는 것은 창의 묶음 멤버(머신마다 메인
// 체크아웃)와 지금 활성 체크아웃이다. 고른 머신이 묶음에서 빠지면 활성 체크아웃으로 돌아간다(`resolveTarget`).
//
// 고른 체크아웃의 실행 · 셸은 그 머신 연결로 간다(`useCheckoutHost` — M7-11 의 부품을 그대로 쓴다). 이 머신이 아니면 단추에 그 머신
// 이름이 글로 보인다(색만으로 가르지 않는다 — M7-11 의 머신 표지).

import { Check, ChevronDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Checkout, checkoutKey, checkoutOfKey, LOCAL_MACHINE } from "../checkout/checkout";
import { useCheckoutHosts } from "../checkout/checkout-hosts";
import { MachineMark, useMachineName } from "../checkout/machine-mark";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { activeCheckout, activeWorktreeStore } from "../open-with/active-worktree";
import { useRunStrings } from "./strings";

/** 고른 대상. `undefined` 면 활성 체크아웃을 따라간다. */
export interface TargetState {
  chosen: Checkout | undefined;
}

export function createTargetStore(): StoreApi<TargetState> {
  return createStore<TargetState>()(() => ({ chosen: undefined }));
}

/** 두 체크아웃이 같은가. */
export function sameCheckout(a: Checkout, b: Checkout): boolean {
  return a.machine === b.machine && (a.worktree ?? undefined) === (b.worktree ?? undefined);
}

/**
 * 지금 대상: 고른 것이 아직 고를 수 있으면 그것, 아니면 활성 체크아웃. `machines` 는 창이 아는 머신(묶음 멤버) — 비었으면(묶음이 아닌
 * 창 · 아직 못 읽음) 이 머신만 안다.
 */
export function resolveTarget(
  chosen: Checkout | undefined,
  active: Checkout,
  machines: readonly string[],
): Checkout {
  if (!chosen) return active;
  const known = machines.length === 0 ? [LOCAL_MACHINE] : machines;
  return known.includes(chosen.machine) ? chosen : active;
}

/** 고를 수 있는 체크아웃: 지금 활성 체크아웃, 그리고 머신마다 메인 체크아웃(겹치면 하나). */
export function targetChoices(active: Checkout, machines: readonly string[]): Checkout[] {
  const out: Checkout[] = [active];
  const known = machines.length === 0 ? [LOCAL_MACHINE] : machines;
  for (const machine of known) {
    const main = { machine };
    if (!out.some((c) => sameCheckout(c, main))) out.push(main);
  }
  return out;
}

/** 창이 아는 머신(묶음 멤버). 체크아웃 공급자 밖이면 빔. */
export function useWindowMachines(): string[] {
  const hosts = useCheckoutHosts();
  const members = useStore(
    hosts?.store ?? EMPTY,
    (s) => (s as { directory?: { members: ReadonlyMap<string, string> } }).directory?.members,
  );
  return members ? [...members.keys()] : [];
}

const EMPTY = createStore<object>()(() => ({}));

/** 지금 활성 체크아웃(화면이 구독). */
export function useActiveCheckout(): Checkout {
  const key = useStore(activeWorktreeStore, (s) => s.top);
  return checkoutOfKey(key);
}

/** 지금 대상(화면이 구독). */
export function useTarget(store: StoreApi<TargetState>): Checkout {
  const chosen = useStore(store, (s) => s.chosen);
  return resolveTarget(chosen, useActiveCheckout(), useWindowMachines());
}

/** 키를 누른 때의 대상(액션 · 띄우기). `machines` 는 창의 묶음 멤버. */
export function currentTarget(store: StoreApi<TargetState>, machines: readonly string[]): Checkout {
  return resolveTarget(store.getState().chosen, activeCheckout(), machines);
}

/** 체크아웃의 사람 이름(「mini · 메인 체크아웃」, 「이 컴퓨터 · wt1」). */
export function useCheckoutLabel(): (checkout: Checkout) => string {
  const T = useRunStrings().picker;
  const name = useMachineName();
  return (c) => T.checkout(name(c.machine), c.worktree?.split(/[\\/]/).pop());
}

/**
 * 대상 체크아웃 단추와 그 고르기 목록. 단추는 대상이 이 머신이 아니면 그 머신 이름을 글로 보인다. 목록의 첫 줄은 「활성 체크아웃
 * 따라가기」 다. 목록이 떠 있는 동안 창의 키맵은 쉰다.
 */
export function TargetChooser({
  store,
  label,
  testId,
}: {
  store: StoreApi<TargetState>;
  /** 단추 · 목록의 접근성 이름. */
  label: string;
  testId: string;
}) {
  const T = useRunStrings().picker;
  const [open, setOpen] = useState(false);
  const chosen = useStore(store, (s) => s.chosen);
  const active = useActiveCheckout();
  const machines = useWindowMachines();
  const target = resolveTarget(chosen, active, machines);
  const labelOf = useCheckoutLabel();
  const choices = targetChoices(active, machines);
  // 머신이 하나뿐인 창(묶음이 아님)은 고를 것이 없다 — 단추를 그리지 않는다.
  if (machines.length <= 1 && chosen === undefined && active.machine === LOCAL_MACHINE) return null;
  const pick = (checkout: Checkout | undefined) => {
    store.setState({ chosen: checkout });
    setOpen(false);
  };
  return (
    <span className="relative flex items-center">
      <button
        type="button"
        data-testid={testId}
        data-target={checkoutKey(target) ?? ""}
        aria-label={`${label}: ${labelOf(target)}`}
        title={`${label}: ${labelOf(target)}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-0.5 rounded-sm px-1 py-0.5 hover:bg-ui-hover"
      >
        {target.machine === LOCAL_MACHINE ? (
          <span className="text-[11px] text-ui-muted-foreground">{labelOf(target)}</span>
        ) : (
          <MachineMark machine={target.machine} />
        )}
        <ChevronDown size={11} aria-hidden />
      </button>
      {open && (
        <TargetMenu
          label={label}
          onClose={() => setOpen(false)}
          items={[
            {
              key: "active",
              text: T.targetActive(labelOf(active)),
              checked: chosen === undefined,
              onSelect: () => pick(undefined),
            },
            ...choices.map((c) => ({
              key: checkoutKey(c) ?? "",
              text: labelOf(c),
              machine: c.machine,
              checked: chosen !== undefined && sameCheckout(chosen, c),
              onSelect: () => pick(c),
            })),
          ]}
        />
      )}
    </span>
  );
}

function TargetMenu({
  label,
  items,
  onClose,
}: {
  label: string;
  items: { key: string; text: string; machine?: string; checked: boolean; onSelect(): void }[];
  onClose(): void;
}) {
  useKeymapPopup(true);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("[role=menuitemradio]")?.focus();
    const outside = (e: MouseEvent) => {
      if (ref.current && !ref.current.parentElement?.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [onClose]);
  return (
    <div
      ref={ref}
      role="menu"
      aria-label={label}
      data-testid="checkout-target-menu"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
          return;
        }
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const all = [...(ref.current?.querySelectorAll<HTMLElement>("[role=menuitemradio]") ?? [])];
        const at = all.indexOf(document.activeElement as HTMLElement);
        const next = all[(at + (e.key === "ArrowDown" ? 1 : all.length - 1)) % all.length];
        next?.focus();
      }}
      className="absolute top-full left-0 z-50 mt-0.5 flex min-w-[220px] flex-col rounded-sm border border-ui-border bg-ui-panel py-0.5 text-[12px] text-ui-foreground shadow-lg"
    >
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="menuitemradio"
          aria-checked={item.checked}
          data-checkout={item.key}
          data-machine={item.machine}
          onClick={item.onSelect}
          className="flex items-center gap-1.5 px-2 py-1 text-left hover:bg-ui-hover focus:bg-ui-hover focus:outline-none"
        >
          <Check size={11} aria-hidden className={item.checked ? "" : "invisible"} />
          <span className="truncate">{item.text}</span>
        </button>
      ))}
    </div>
  );
}
