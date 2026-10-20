// 「외부 도구 > Orca 에서 Pitwall 열기」(docs/design/04-features.md F8-6): Orca 의 Open in 메뉴에 등록할 값과, 그 명령이 이 앱을
// 띄우는지의 검사(`cli.status`, crates/pw-open-with/src/cli.rs). 검사가 맞지 않으면 「Shell 명령 설치」 나 절대 경로 등록을
// 안내한다(04 「Orca "Open in" 동작」 표의 PATH 행).

import type { CliStatus } from "@pitwall/rpc-client";
import { useState } from "react";

import { useRpcQuery } from "../rpc/host";
import type { SettingsNative } from "./native";
import { type SettingsStrings, useSettingsStrings } from "./strings";

/** Orca 에 등록할 라벨과 명령(04 「Orca 연동 범위」 표). */
export const ORCA_LABEL = "Pitwall";
export const ORCA_COMMAND = "pitwall";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const NOTE = "text-[12px] text-ui-muted-foreground";

/** 오류의 글. RPC 오류는 `data` 에 이유가 있다(`internal error (-32603)` 만으로는 무엇이 실패했는지 모른다). */
function errorText(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const data = "data" in e && typeof e.data === "string" ? `: ${e.data}` : "";
    return `${String(e.message)}${data}`;
  }
  return String(e);
}

/** 값 하나와 그 복사 버튼. 복사한 뒤 잠깐 「복사했다」 를 보인다. */
function CopyValue({
  name,
  value,
  native,
  T,
}: {
  name: string;
  value: string;
  native: SettingsNative;
  T: SettingsStrings["orca"];
}) {
  const [state, setState] = useState<
    { kind: "idle" | "copied" } | { kind: "failed"; error: string }
  >({ kind: "idle" });
  return (
    <div className="flex items-center gap-3">
      <span className="w-14 text-ui-muted-foreground">{name}</span>
      <code className="rounded-sm bg-ui-panel px-1.5 py-0.5 select-all">{value}</code>
      <button
        type="button"
        className={BUTTON}
        aria-label={T.copyValue(value)}
        onClick={() => {
          native.copyText(value).then(
            () => setState({ kind: "copied" }),
            (e: unknown) => setState({ kind: "failed", error: errorText(e) }),
          );
        }}
      >
        {T.copy}
      </button>
      {state.kind === "copied" && (
        <span role="status" className={NOTE}>
          {T.copied}
        </span>
      )}
      {state.kind === "failed" && (
        <span role="alert" className="text-[12px] text-ui-accent">
          {T.copyFailed(state.error)}
        </span>
      )}
    </div>
  );
}

/** 설치 위치 · PATH 의 판정을 한 줄 머리와 설명으로. */
export function describeStatus(status: CliStatus, T: SettingsStrings["orca"]) {
  const { install, onPath, target } = status;
  const headline = `${T.install[install.kind]} · ${T.onPath[onPath.kind]}`;
  const details: string[] = [];
  switch (install.kind) {
    case "thisApp":
      details.push(T.detail.installed(target));
      break;
    case "missing":
      details.push(T.detail.missing(target));
      break;
    case "otherLink":
      details.push(T.detail.otherLink(target, install.target));
      break;
    case "notALink":
      details.push(T.detail.notALink(target));
      break;
  }
  if (onPath.kind === "other") {
    details.push(T.detail.otherOnPath(onPath.path));
    if (onPath.resolved !== null) details.push(T.detail.resolved(onPath.resolved));
  }
  return {
    headline,
    details,
    /** 「Shell 명령 설치」 가 고칠 수 있는가. 심링크가 아닌 파일은 설치가 건드리지 않는다. */
    canInstall: install.kind === "missing" || install.kind === "otherLink",
    /** 이 앱으로 설치됐는데 PATH 로는 이 앱이 잡히지 않는다 — 절대 경로를 등록한다. */
    needsAbsolute: install.kind === "thisApp" && onPath.kind !== "thisApp",
    fallback: status.pathStatus.kind === "fallback" ? status.pathStatus.reason : undefined,
  };
}

export function OrcaSection({ native }: { native: SettingsNative }) {
  const T = useSettingsStrings().orca;
  const status = useRpcQuery("cli.status", null, { retry: false });
  const [installing, setInstalling] = useState(false);
  const install = native.installShellCommand;
  const view = status.data ? describeStatus(status.data, T) : undefined;

  const runInstall = () => {
    if (!install) return;
    setInstalling(true);
    // 결과(성공 · 실패)는 네이티브 대화상자가 알린다. 취소 · 실패로 거부돼도 디스크는 다시 본다.
    install()
      .catch(() => {})
      .finally(() => {
        setInstalling(false);
        void status.refetch();
      });
  };

  return (
    <section aria-labelledby="settings-orca" className="flex flex-col gap-2">
      <h3 id="settings-orca" className="font-semibold">
        {T.title}
      </h3>
      <p className={NOTE}>{T.intro}</p>
      <CopyValue name={T.label} value={ORCA_LABEL} native={native} T={T} />
      <CopyValue name={T.command} value={ORCA_COMMAND} native={native} T={T} />

      <div role="status" aria-live="polite" className="mt-1 flex flex-col gap-1">
        {status.isPending && <span className={NOTE}>{T.checking}</span>}
        {status.isError && (
          <span role="alert" className="text-[12px] text-ui-accent">
            {T.checkFailed(errorText(status.error))}
          </span>
        )}
        {view && (
          <>
            <span data-testid="orca-headline" className="font-semibold">
              {view.headline}
            </span>
            {view.details.map((d) => (
              <span key={d} className={NOTE}>
                {d}
              </span>
            ))}
            {view.fallback !== undefined && (
              <span className={NOTE}>{T.pathFallback(view.fallback)}</span>
            )}
          </>
        )}
      </div>

      <div className="flex items-center gap-2">
        {view?.canInstall && install && (
          <button type="button" className={BUTTON} disabled={installing} onClick={runInstall}>
            {installing ? T.installing : T.installButton}
          </button>
        )}
        <button
          type="button"
          className={BUTTON}
          disabled={status.isFetching}
          onClick={() => void status.refetch()}
        >
          {T.recheck}
        </button>
      </div>

      {view?.needsAbsolute && status.data && (
        <div className="flex flex-col gap-1">
          <span className={NOTE}>{T.absolute(status.data.target)}</span>
          <CopyValue name={T.command} value={status.data.target} native={native} T={T} />
        </div>
      )}

      <p className={NOTE}>{T.worktree}</p>
    </section>
  );
}
