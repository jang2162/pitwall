// 「체크아웃 추가」 화면(18-m7-plan M7-11b ①, 08-remote-mobile.md 「묶음」 — 멤버를 더하는 것은 사람이 한다). 브랜치 팝업의
// 「+ 원격 체크아웃 추가…」(프로젝트 창 — 그 창의 묶음에 더한다)와 웰컴 창 원격 호스트의 [체크아웃 추가…](원격만인 새 묶음)가 띄운다.
//
// - 머신과 그 머신의 경로를 골라 `bundle.addMember` 를 부른다. 판정(같은 저장소인지 — 공통 커밋)은 Host 작업으로 돌고 상한
//   (`JUDGE_TIMEOUT`, 180초)이 있다(M7-11 — pw-core `registry::bundle::add`). 도는 동안 진행 문장과 [취소] 를 보인다. [취소] 는 그
//   요청을 취소한다(`request.cancel` — Host 가 작업을 멈추고 읽기를 버린다). 진행 단계는 상태바의 작업 표시가 보인다.
// - 결과 · 거절은 사람의 말로 옮긴다(`addOutcomeText`). 시간 상한의 거절(`judgeTimedOut`)과 취소(`cancelled`)도 그렇다. 파일이 많은
//   경로(`largeDirectory`)는 [그대로 더하기] 로 다시 부른다(`allowLarge`).
// - **고른 머신이 곧 보내는 머신이다**: 머신 목록의 값은 머신 id(`remote.hosts.list` 의 기억한 id)이고 화면에는 그 이름을 보인다.
//   연결해 본 적 없는 호스트(머신 id 를 모른다)는 목록에 없다 — 이름이 같은 다른 머신으로 더해지지 않게. 이미 그 묶음에 멤버가 있는
//   머신도 빠진다(한 머신에 멤버 하나 — M7-P20).
// - 고른 머신은 대화상자가 떠 있는 동안 링크를 쥔다(`remote.connect` — 창의 로컬 Host 범위 연결, M7-08). 웰컴 창의 원격 호스트 줄은
//   이미 그 호스트를 쓰는 중이라 쥐지 않는다.

import {
  type AddMemberOutcome,
  ERROR_CODE,
  type ProjectBundle,
  type RemoteStatus,
  type RpcClient,
  RpcCallError,
} from "@pitwall/rpc-client";
import type { Action } from "@pitwall/keymap";
import * as Dialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useRef, useState } from "react";

import { useAppStrings } from "../i18n/app-strings";
import { useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { useNotify } from "../notifications/notifications";
import { useRpcQueryOn } from "../rpc/host";
import { LOCAL_MACHINE } from "./checkout";
import { useLinkHost, useLocalHost, type WindowProject } from "./checkout-hosts";
import { useMachineName } from "./machine-mark";

/** 브랜치 팝업의 「+ 원격 체크아웃 추가…」 가 부르는 액션(git/branch-popup.tsx). */
export const ADD_CHECKOUT_ACTION = "checkout.addRemote";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50";
const FIELD =
  "rounded-sm border border-ui-border bg-ui-background px-2 py-1 outline-none focus:border-ui-accent";

type Strings = ReturnType<typeof useAppStrings>["checkoutAdd"];

/** 한 번 부른 결과의 화면 꼴. `added` 면 대화상자가 닫힌다. */
export type AddResult =
  | { kind: "added"; bundle: ProjectBundle }
  | { kind: "large"; atLeast: number; text: string }
  | { kind: "refused"; text: string };

/** `bundle.addMember` 의 답 · 오류를 사람의 말로(`name` 은 머신 id → 이름). */
export function addOutcomeText(
  done: { outcome: AddMemberOutcome } | { error: unknown },
  T: Strings,
  name: (machine: string) => string,
): AddResult {
  if ("outcome" in done) {
    const o = done.outcome;
    if (o.kind === "added") return { kind: "added", bundle: o.bundle };
    if (o.kind === "largeDirectory")
      return { kind: "large", atLeast: o.atLeast, text: T.large(o.atLeast) };
    const r = o.reason;
    const text = (() => {
      switch (r.kind) {
        case "notGitRepository":
          return T.notGit;
        case "noCommits":
          return T.noCommits;
        case "secondClone":
          return T.secondClone;
        case "alreadyCheckout":
          return T.already;
        case "noCommonCommit":
          return T.noCommon(name(r.machine));
        case "shallowUnknown":
          return T.shallow(name(r.machine));
        case "inOtherBundle":
          return T.otherBundle;
        case "bundleChanged":
          return T.changed;
      }
    })();
    return { kind: "refused", text };
  }
  const e = done.error;
  if (e instanceof RpcCallError) {
    const data = (e.data ?? {}) as { reason?: unknown; seconds?: unknown };
    if (e.code === ERROR_CODE.CANCELLED && data.reason === "judgeTimedOut")
      return {
        kind: "refused",
        text: T.timedOut(typeof data.seconds === "number" ? data.seconds : 0),
      };
    if (e.code === ERROR_CODE.CANCELLED) return { kind: "refused", text: T.cancelled };
    if (data.reason === "machineUnavailable") return { kind: "refused", text: T.unavailable };
    return { kind: "refused", text: T.failed(e.message) };
  }
  return { kind: "refused", text: T.failed(e instanceof Error ? e.message : String(e)) };
}

/** 고를 수 있는 머신: 머신 id 를 아는 원격 호스트 가운데 `taken`(이미 묶음 멤버인 머신)이 아닌 것. 한 id 는 한 번(앞 이름). */
export function machineChoices(
  hosts: readonly { name: string; machine: string | null }[],
  taken: ReadonlySet<string>,
): { machine: string; name: string }[] {
  const seen = new Set<string>();
  const out: { machine: string; name: string }[] = [];
  for (const h of hosts) {
    if (h.machine === null || h.machine === LOCAL_MACHINE || taken.has(h.machine)) continue;
    if (seen.has(h.machine)) continue;
    seen.add(h.machine);
    out.push({ machine: h.machine, name: h.name });
  }
  return out;
}

/**
 * 취소한 더하기가 그래도 붙었는가: 더하기 전 묶음(`before`)의 멤버를 모두 가진 지금 묶음에 그 머신의 멤버가 있으면 그 묶음. 암묵
 * 묶음은 붙으면 id 가 바뀌므로 id 가 아니라 멤버로 찾는다.
 */
export function joinedAnyway(
  bundles: readonly ProjectBundle[],
  before: ProjectBundle,
  machine: string,
): ProjectBundle | undefined {
  return bundles.find(
    (b) =>
      before.members.every((m) =>
        b.members.some((n) => n.machine === m.machine && n.projectId === m.projectId),
      ) && b.members.some((n) => n.machine === machine),
  );
}

/** 대화상자가 떠 있는 동안 그 호스트의 링크를 쥔다(`remote.connect`). 상태를 돌려준다. StrictMode 의 열기 · 닫기는 짝이다. */
function useHeldLink(client: RpcClient | undefined, name: string | undefined) {
  const [status, setStatus] = useState<RemoteStatus>();
  useEffect(() => {
    setStatus(undefined);
    if (!client || name === undefined) return;
    const abort = new AbortController();
    void (async () => {
      try {
        for await (const s of client.stream("remote.connect", { name }, { signal: abort.signal }))
          if (!abort.signal.aborted) setStatus(s);
      } catch (e) {
        if (!abort.signal.aborted) console.warn("[checkout] the link stream ended", name, e);
      }
    })();
    return () => abort.abort();
  }, [client, name]);
  return status;
}

export function AddCheckoutDialog({
  client,
  bundle,
  machine: fixed,
  onClose,
  onAdded,
}: {
  /** `bundle.addMember` 를 부를 연결(창의 이 머신 연결). */
  client: RpcClient | undefined;
  /** 더할 묶음. 없으면 원격만인 새 묶음(웰컴 창의 원격 호스트). */
  bundle: ProjectBundle | undefined;
  /** 머신을 정해 두고 연다(원격 호스트 줄). 없으면 고른다. */
  machine?: string;
  onClose(): void;
  onAdded(bundle: ProjectBundle): void;
}) {
  const T = useAppStrings().checkoutAdd;
  const name = useMachineName();
  useKeymapPopup(true);
  const link = useLinkHost();
  const hosts = useRpcQueryOn(link, "remote.hosts.list", null, { retry: false });
  const taken = new Set(bundle?.members.map((m) => m.machine));
  const choices = machineChoices(hosts.data?.hosts ?? [], taken);
  const [picked, setPicked] = useState<string>();
  const machine = fixed ?? picked ?? choices[0]?.machine;
  const [path, setPath] = useState("");
  const [running, setRunning] = useState<AbortController>();
  const [result, setResult] = useState<Exclude<AddResult, { kind: "added" }>>();
  const hostName = fixed ? undefined : choices.find((c) => c.machine === machine)?.name;
  const status = useHeldLink(link.client, hostName);
  const connecting = hostName !== undefined && status?.state !== "connected";
  // 닫힌 뒤(취소 · 바깥 누르기) 늦게 온 답으로 상태를 바꾸지 않는다.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const ids = { machine: useId(), path: useId() };

  async function add(allowLarge: boolean) {
    if (!client || machine === undefined || path.trim() === "") return;
    const abort = new AbortController();
    setRunning(abort);
    setResult(undefined);
    const done = await client
      .call(
        "bundle.addMember",
        {
          bundleId: bundle?.id ?? null,
          machine,
          path: path.trim(),
          allowLarge: allowLarge || null,
        },
        { signal: abort.signal },
      )
      .then(
        (outcome) => ({ outcome }),
        (error: unknown) => ({ error }),
      );
    if (!alive.current) return;
    let shown = addOutcomeText(done, T, name);
    // 붙이기 단계(3/3)에 든 뒤의 취소는 요청만 끝낸다 — 멤버는 붙는다(M7-11). 그때 「취소했다」 라고만 말하지 않는다(리뷰 R1).
    if (shown.kind === "refused" && shown.text === T.cancelled && bundle) {
      const now = await client.call("bundle.list", null).catch(() => undefined);
      if (!alive.current) return;
      if (now && joinedAnyway(now, bundle, machine))
        shown = { kind: "refused", text: T.cancelledButAdded };
    }
    setRunning(undefined);
    if (shown.kind === "added") onAdded(shown.bundle);
    else setResult(shown);
  }

  const close = () => {
    running?.abort();
    onClose();
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="add-checkout-dialog"
          className="sheet-top fixed left-1/2 flex w-[520px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-[12px] text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">
            {bundle ? T.into(bundle.name) : T.newBundle}
          </Dialog.Description>
          <form
            className="flex flex-col gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!running && !connecting) void add(false);
            }}
          >
            <label htmlFor={ids.machine} className="text-ui-muted-foreground">
              {T.machine}
            </label>
            {fixed !== undefined ? (
              <span id={ids.machine} data-testid="add-checkout-machine" data-machine={fixed}>
                {name(fixed)}
              </span>
            ) : choices.length === 0 ? (
              <span id={ids.machine} role="status">
                {T.noMachines}
              </span>
            ) : (
              <select
                id={ids.machine}
                className={FIELD}
                value={machine}
                disabled={running !== undefined}
                onChange={(e) => setPicked(e.target.value)}
              >
                {choices.map((c) => (
                  <option key={c.machine} value={c.machine}>
                    {name(c.machine)}
                  </option>
                ))}
              </select>
            )}
            <label htmlFor={ids.path} className="text-ui-muted-foreground">
              {T.path}
            </label>
            <input
              id={ids.path}
              autoFocus
              className={`${FIELD} font-mono`}
              value={path}
              placeholder={T.pathHint}
              spellCheck={false}
              disabled={running !== undefined}
              onChange={(e) => setPath(e.target.value)}
            />
          </form>
          {running && machine !== undefined && (
            <p role="status" data-testid="add-checkout-running">
              {T.running(name(machine))}
            </p>
          )}
          {!running && connecting && machine !== undefined && (
            <p role="status">{T.connecting(name(machine))}</p>
          )}
          {result && (
            <p role="alert" data-testid="add-checkout-result" className="text-ui-accent">
              {result.text}
            </p>
          )}
          <div className="flex justify-end gap-2">
            {running ? (
              // 도는 동안의 [취소] 는 그 요청을 취소한다 — 대화상자는 남아 결과(「취소했다」)를 보인다.
              <button type="button" className={BUTTON} onClick={() => running.abort()}>
                {T.cancel}
              </button>
            ) : (
              <>
                <button type="button" className={BUTTON} onClick={close}>
                  {T.close}
                </button>
                {result?.kind === "large" && (
                  <button type="button" className={BUTTON} onClick={() => void add(true)}>
                    {T.allowLarge}
                  </button>
                )}
                <button
                  type="button"
                  className={BUTTON}
                  disabled={!client || machine === undefined || path.trim() === "" || connecting}
                  onClick={() => void add(false)}
                >
                  {T.submit}
                </button>
              </>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** `bundle.list` 쿼리를 모두 다시 읽게 한다(어느 연결의 것이든 — 창의 묶음 · 웰컴 줄). */
export function useRefreshBundles(): () => void {
  const queryClient = useQueryClient();
  return () =>
    void queryClient.invalidateQueries({ predicate: (q) => q.queryKey[2] === "bundle.list" });
}

/**
 * 프로젝트 창의 「원격 체크아웃 추가…」 액션과 그 대화상자. 그 창의 묶음에 더한다(암묵 묶음이면 그 로컬 프로젝트 id — 행 있는 묶음이
 * 되면 묶음 id 가 바뀌지만 창은 그대로다, src-tauri/src/launch.rs `open_bundle_as`). 창이 사는 동안 한 번 둔다.
 */
export function AddCheckoutFeature({
  windowProject,
}: {
  windowProject: WindowProject | undefined;
}) {
  const T = useAppStrings().checkoutAdd;
  const notify = useNotify();
  const local = useLocalHost();
  const refresh = useRefreshBundles();
  const [open, setOpen] = useState(false);
  const bundle = windowProject?.bundle;
  const actions = useMemo<Action[]>(
    () => [
      {
        id: ADD_CHECKOUT_ACTION,
        label: T.action,
        keywords: ["Add Remote Checkout", "Add Checkout"],
        enabled: () => bundle !== undefined,
        run: () => setOpen(true),
      },
    ],
    [T, bundle],
  );
  useRegisterActions(actions);
  if (!open || !bundle) return null;
  return (
    <AddCheckoutDialog
      client={local.client}
      bundle={bundle}
      onClose={() => setOpen(false)}
      onAdded={() => {
        setOpen(false);
        refresh();
        notify({ level: "info", title: T.added });
      }}
    />
  );
}
