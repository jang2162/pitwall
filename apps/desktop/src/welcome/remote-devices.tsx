// 웰컴 「원격 호스트」 탭의 기기 칸(18-m7-plan M7-23 ③): 붙은 호스트 줄에서 그 Relay 의 페어링 표를 본다 — 기기 이름 · role ·
// 페어링 시각 · 마지막 접속, [해제], 다른 기기를 붙일 [코드 띄우기].
//
// - 모두 Desktop Host 의 중계(`remote.devices` · `remote.unpairDevice` · `remote.pairCode`)로 부르고, 사람이 본 머신 id 를 싣는다 —
//   그 호스트가 지금 다른 머신에 붙어 있으면 Host 가 부르지 않는다. role 판정은 Relay 가 한다(owner 기기만).
// - 해제는 한 번 더 묻는다. 이 Desktop 이 그 Relay 의 마지막 owner 기기면, 해제한 뒤 그 Relay 에 닿는 길은 SSH 로 다시 페어링하는
//   것뿐이라는 경고를 보인다(막지는 않는다 — 메인 판정).
// - 코드는 화면에만 보인다(로그 · 콘솔에 쓰지 않는다). 칸을 닫으면 잊는다.

import type { RelayDevice, RelayPairCode, Role } from "@pitwall/rpc-client";
import { useMemo, useState } from "react";

import { useI18n } from "../i18n/i18n-provider";
import { useHost, useRpcQuery } from "../rpc/host";
import type { WelcomeStrings } from "./strings";

const LOG = "[remote-devices]";
const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const ROLES: readonly Role[] = ["readonly", "mobile", "owner"];

/** 이 기기를 해제하면 그 Relay 에 owner 기기가 남지 않는가. */
export function isLastOwner(devices: readonly RelayDevice[], device: RelayDevice): boolean {
  return device.role === "owner" && devices.filter((d) => d.role === "owner").length === 1;
}

/** `123456` → `123 456`(읽기 쉽게 — 받는 쪽은 공백을 뺀다). */
export function spacedCode(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

export function RemoteDevices({
  name,
  machine,
  T,
  onError,
}: {
  name: string;
  machine: string;
  T: WelcomeStrings["remote"]["devices"];
  onError(message: string): void;
}) {
  const { client } = useHost();
  const devices = useRpcQuery("remote.devices", { name, machine }, { retry: false });
  const { locales } = useI18n();
  const when = useMemo(
    () => new Intl.DateTimeFormat(locales[0], { dateStyle: "short", timeStyle: "short" }),
    [locales],
  );
  const [asking, setAsking] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [role, setRole] = useState<Role>("readonly");
  const [code, setCode] = useState<RelayPairCode>();
  const failed = (method: string) => (e: unknown) => {
    onError(String(e instanceof Error ? e.message : e));
    console.warn(LOG, method, "failed");
  };
  const list = devices.data?.devices ?? [];
  const unpair = (device: RelayDevice) => {
    setBusy(true);
    setAsking(undefined);
    client
      ?.call("remote.unpairDevice", { name, machine, device: device.device })
      .catch(failed("remote.unpairDevice"))
      .finally(() => {
        setBusy(false);
        void devices.refetch();
      });
  };
  const issue = () => {
    setBusy(true);
    client
      ?.call("remote.pairCode", { name, machine, role, deviceName: null })
      .then(setCode)
      .catch(failed("remote.pairCode"))
      .finally(() => setBusy(false));
  };
  return (
    <section
      aria-label={T.title}
      data-testid="remote-devices"
      className="flex flex-col gap-1 border-t border-ui-border pt-1 text-[12px]"
    >
      {devices.isError && <div role="alert">{T.unreadable}</div>}
      <ul className="flex flex-col gap-0.5">
        {list.map((device) => {
          const last = isLastOwner(list, device);
          return (
            <li key={device.device} data-device={device.device} className="flex flex-col">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-semibold">{device.name || T.unnamed}</span>
                {device.you && <span className="text-ui-accent">{T.you}</span>}
                <span className="rounded-sm border border-ui-border px-1 text-[11px]">
                  {T.role[device.role]}
                </span>
                <span className="text-ui-muted-foreground">
                  {T.pairedAt(
                    device.pairedAtMs === null ? T.unknown : when.format(device.pairedAtMs),
                  )}
                </span>
                <span className="text-ui-muted-foreground">
                  {T.lastSeen(
                    device.lastSeenMs === null ? T.never : when.format(device.lastSeenMs),
                  )}
                </span>
                <span
                  className="truncate font-mono text-[11px] text-ui-muted-foreground"
                  title={device.key}
                >
                  {device.device}
                </span>
                {asking !== device.device && (
                  <button
                    type="button"
                    className={BUTTON}
                    disabled={busy}
                    onClick={() => setAsking(device.device)}
                  >
                    {T.unpair}
                  </button>
                )}
              </div>
              {asking === device.device && (
                <div
                  role="alertdialog"
                  aria-label={T.confirmTitle}
                  className="flex flex-wrap items-center gap-2 pl-2"
                >
                  <span className={last && device.you ? "text-ui-accent" : ""}>
                    {last && device.you
                      ? T.confirmLastOwner
                      : device.you
                        ? T.confirmSelf
                        : T.confirm(device.name || T.unnamed)}
                  </span>
                  <button
                    type="button"
                    className={BUTTON}
                    autoFocus
                    onClick={() => setAsking(undefined)}
                  >
                    {T.cancel}
                  </button>
                  <button
                    type="button"
                    className={BUTTON}
                    disabled={busy}
                    onClick={() => unpair(device)}
                  >
                    {T.unpairConfirm}
                  </button>
                </div>
              )}
            </li>
          );
        })}
      </ul>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-1">
          {T.codeRole}
          <select
            value={role}
            disabled={busy}
            onChange={(e) => setRole(e.currentTarget.value as Role)}
            className="rounded-sm border border-ui-border bg-ui-panel"
          >
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {T.role[r]}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className={BUTTON} disabled={busy} onClick={issue}>
          {T.showCode}
        </button>
      </div>
      {code && (
        <div data-testid="pairing-code" className="flex flex-col gap-0.5">
          <span className="font-mono text-[20px] tracking-widest">{spacedCode(code.code)}</span>
          <span className="text-ui-muted-foreground">
            {T.codeHint(when.format(code.expiresAtMs))}
          </span>
        </div>
      )}
    </section>
  );
}
