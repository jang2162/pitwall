// 프로젝트 신뢰(M1-05, F1-7 최소판): 처음 여는 프로젝트의 확인 다이얼로그(10-ui 「다이얼로그 · 팝업 목록」 「기타」 행), 상태바
// 「런타임 환경 미적용(신뢰 필요)」, 신뢰 · 해제 액션. 신뢰는 Host 의 레지스트리에 있다(crates/pw-core/src/registry/trust.rs) —
// 워크트리 프로젝트는 메인 저장소 프로젝트의 신뢰를 따르고, 미신뢰면 런타임 제공자(mise 등)를 돌리지 않는다.
//
// 다이얼로그는 한 번만 뜬다: 창은 띄우기 전에 `project.claimTrustPrompt` 를 부르고, Host 가 「묻지 않음」 을 「미신뢰」 로 바꾼
// 호출에만 `true` 를 준다 — 같은 프로젝트의 다른 창이 먼저 물었거나 이 창이 다시 불러도(재연결) 두 번 뜨지 않는다. 답하지
// 않고 닫으면 미신뢰로 남고 상태바 항목에서 다시 연다.

import type { Action } from "@pitwall/keymap";
import { RpcCallError } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { ShieldAlert } from "lucide-react";
import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useKeymap, useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { type NotificationInput, useNotify } from "../notifications/notifications";
import { useHost, useRpcQuery } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

/** 액션 id. 상태바 항목은 신뢰 액션을 부른다(status-bar.tsx 머리 주석). */
export const TRUST_ACTION = "project.trust";
export const UNTRUST_ACTION = "project.untrust";

/** 오류 알림의 본문. 저장소 오류(`STORAGE_UNAVAILABLE`)는 `data` 에 이유가 있다. */
function errorBody(e: unknown): string {
  if (e instanceof RpcCallError && typeof e.data === "string") return e.data;
  return e instanceof Error ? e.message : String(e);
}

export function trustActions(deps: {
  strings: ProjectStrings["trust"];
  /** 지금 신뢰하는가. 모르면(읽기 전 · 실패) `undefined` — 두 액션 다 꺼진다. */
  trusted(): boolean | undefined;
  /** 확인 다이얼로그를 연다. 신뢰는 다이얼로그의 버튼으로만 한다. */
  openDialog(): void;
  setTrust(trusted: boolean): Promise<void>;
  notify(input: NotificationInput): void;
}): Action[] {
  const T = deps.strings;
  return [
    {
      id: TRUST_ACTION,
      label: T.trustAction,
      keywords: ["Trust Project", "Workspace Trust", "신뢰"],
      enabled: () => deps.trusted() === false,
      run: () => deps.openDialog(),
    },
    {
      id: UNTRUST_ACTION,
      label: T.untrustAction,
      keywords: ["Untrust Project", "Restricted Mode", "신뢰"],
      enabled: () => deps.trusted() === true,
      run: () =>
        deps
          .setTrust(false)
          .catch((e: unknown) =>
            deps.notify({ level: "error", title: T.saveFailed, body: errorBody(e) }),
          ),
    },
  ];
}

interface TrustContextValue {
  projectId: string;
  trusted: boolean | undefined;
}

const TrustContext = createContext<TrustContextValue | undefined>(undefined);

/** 상태바 항목: 신뢰하지 않은 프로젝트에서만 「런타임 환경 미적용(신뢰 필요)」. 누르면 신뢰 액션(다이얼로그). */
export function TrustStatusItem() {
  const value = useContext(TrustContext);
  const T = useProjectStrings().trust;
  const { registry } = useKeymap();
  if (value?.trusted !== false) return null;
  return (
    <button
      type="button"
      data-testid="status-trust"
      title={T.statusHint}
      onClick={() => registry.run(TRUST_ACTION)}
      className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
    >
      <ShieldAlert size={11} aria-hidden />
      {T.status}
    </button>
  );
}

function TrustDialog({
  name,
  busy,
  onAnswer,
}: {
  name: string;
  busy: boolean;
  onAnswer(trusted: boolean): void;
}) {
  const T = useProjectStrings().trust;
  useKeymapPopup(true);
  return (
    // 바깥을 누르거나 Esc 로 닫으면 「신뢰하지 않음」 이다 — 이미 미신뢰로 기록돼 있다.
    <Dialog.Root open onOpenChange={(open) => !open && !busy && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="trust-dialog"
          className="sheet-top fixed left-1/2 flex w-[520px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title(name)}</Dialog.Title>
          <Dialog.Description className="whitespace-pre-line text-ui-muted-foreground">
            {T.body}
          </Dialog.Description>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={BUTTON}
              disabled={busy}
              onClick={() => onAnswer(false)}
            >
              {T.distrust}
            </button>
            <button
              type="button"
              autoFocus
              className={`${BUTTON} border-ui-accent`}
              disabled={busy}
              onClick={() => onAnswer(true)}
            >
              {T.trust}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 프로젝트 창의 신뢰: 다이얼로그 · 상태바 항목 · 액션. `StatusBarProvider` · `NotificationProvider` 안에 둔다. */
export function TrustProvider({ projectId, children }: { projectId: string; children: ReactNode }) {
  const { client } = useHost();
  const strings = useProjectStrings().trust;
  const notify = useNotify();
  const trust = useRpcQuery("project.trust", { id: projectId });
  const project = useRpcQuery("project.get", { id: projectId });
  const trusted = trust.data ? trust.data.state === "trusted" : undefined;
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  // 연결마다 부른다. 결과는 effect 가 정리된 뒤에도 받는다 — 버리면 그 호출이 받은 단 한 번의 `true` 를 잃는다.
  useEffect(() => {
    if (!client) return;
    client.call("project.claimTrustPrompt", { id: projectId }).then(
      (first) => first && setOpen(true),
      // 저장할 수 없는 레지스트리면 묻지 않는다. 상태바 항목은 그대로 남는다.
      (e: unknown) => console.warn("[trust] cannot claim the trust prompt", e),
    );
  }, [client, projectId]);

  // 액션은 메모해 두고 지금 값은 ref 로 읽는다 — 신뢰 · 연결이 바뀔 때마다 다시 등록하지 않게.
  const latest = useRef<{ trusted: boolean | undefined; save(value: boolean): Promise<void> }>(
    undefined as never,
  );
  latest.current = {
    trusted,
    save: async (value: boolean) => {
      if (!client) throw new Error("not connected to the host");
      await client.call("project.setTrust", { id: projectId, trusted: value });
      // 다른 창은 `project.changed`(kind: trust)로 다시 읽는다. 이 창은 기다리지 않고 바로 읽는다.
      await trust.refetch();
    },
  };
  const actions = useMemo(
    () =>
      trustActions({
        strings,
        notify,
        trusted: () => latest.current.trusted,
        openDialog: () => setOpen(true),
        setTrust: (value) => latest.current.save(value),
      }),
    [strings, notify],
  );
  useRegisterActions(actions);
  useStatusBarItem({ id: "trust", ...STATUS_SLOTS.trust, component: TrustStatusItem });

  const answer = (value: boolean) => {
    setBusy(true);
    latest.current
      .save(value)
      .catch((e: unknown) =>
        notify({ level: "error", title: strings.saveFailed, body: errorBody(e) }),
      )
      .finally(() => {
        setBusy(false);
        setOpen(false);
      });
  };

  const value = useMemo(() => ({ projectId, trusted }), [projectId, trusted]);
  return (
    <TrustContext value={value}>
      {children}
      {open && <TrustDialog name={project.data?.name ?? projectId} busy={busy} onAnswer={answer} />}
    </TrustContext>
  );
}
