// 웰컴 「원격 호스트」 탭의 코드 입력 화면(18-m7-plan M7-24 · M7-23 판정 5): 수동 배치 Relay 에 이 Desktop 을 붙인다.
//
// - [검색](`remote.discover`)은 로컬 Tailscale 의 온라인 기기와 LAN mDNS 광고를 보이고, 그 주소에 Relay 가 받는지(QUIC 핸드셰이크만)
//   표시한다. 고르면 주소 칸이 채워진다 — 사람이 주소를 쳐도 된다.
// - 6자리 코드는 원격 머신의 `pitwall-relay pair`(또는 붙은 Desktop 의 기기 칸)가 보인 것이다. 코드는 이 칸에만 있고 로그 · 콘솔에
//   쓰지 않는다. 페어링(`remote.pairDirect`)은 Noise 위 PAKE 라, 코드를 모르는 Relay(가짜 광고 · 주소 가로채기)와는 되지 않는다.
// - 끝나면 직접 연결 전용 호스트 항목이 목록에 생긴다(그 머신 id 만 받는다).

import type { RemoteDiscovered } from "@pitwall/rpc-client";
import { useState } from "react";

import { useHost } from "../rpc/host";
import type { WelcomeStrings } from "./strings";

const LOG = "[remote-direct]";
const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const FIELD = "rounded-sm border border-ui-border bg-ui-panel px-1";

/** 코드 칸의 값에서 숫자만(`123 456` 도 받는다). */
export function codeDigits(text: string): string {
  return text.replace(/\D/g, "").slice(0, 6);
}

/** 주소에서 기본 이름(점 앞 첫 조각 — `mini.tail1234.ts.net` → `mini`, IP 는 점을 `-` 로). */
export function suggestedName(address: string): string {
  const trimmed = address.trim();
  if (/^[\d.]+$/.test(trimmed) || trimmed.includes(":")) return trimmed.replace(/[.:%]/g, "-");
  return trimmed.split(".")[0] ?? trimmed;
}

export function DirectPair({
  T,
  taken,
  onClose,
  onPaired,
}: {
  T: WelcomeStrings["remote"]["direct"];
  /** 이미 있는 호스트 이름(같은 이름은 고르지 못한다). */
  taken: readonly string[];
  onClose(): void;
  onPaired(name: string): void;
}) {
  const { client } = useHost();
  const [found, setFound] = useState<readonly RemoteDiscovered[]>();
  const [notes, setNotes] = useState<readonly string[]>([]);
  const [searching, setSearching] = useState(false);
  const [address, setAddress] = useState("");
  const [port, setPort] = useState("");
  const [nickname, setNickname] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [done, setDone] = useState<string>();

  const search = () => {
    setSearching(true);
    client
      ?.call("remote.discover", null)
      .then((result) => {
        setFound(result.found);
        setNotes(result.notes);
      })
      .catch((e: unknown) => {
        setError(String(e instanceof Error ? e.message : e));
        console.warn(LOG, "remote.discover failed");
      })
      .finally(() => setSearching(false));
  };
  const pick = (d: RemoteDiscovered) => {
    setAddress(d.address);
    setPort(d.port === 7420 ? "" : String(d.port));
    if (!nickname) setNickname(suggestedName(d.label));
  };
  const name = nickname.trim() || suggestedName(address);
  const digits = codeDigits(code);
  const portNumber = port.trim() === "" ? null : Number(port);
  const portOk =
    portNumber === null || (Number.isInteger(portNumber) && portNumber > 0 && portNumber < 65536);
  const ready =
    address.trim() !== "" && digits.length === 6 && name !== "" && !taken.includes(name) && portOk;
  const pair = () => {
    setBusy(true);
    setError(undefined);
    setDone(undefined);
    client
      ?.call("remote.pairDirect", {
        address: address.trim(),
        port: portNumber,
        code: digits,
        nickname: name,
      })
      .then((result) => {
        setCode("");
        setDone(T.paired(result.name));
        onPaired(result.name);
      })
      .catch((e: unknown) => {
        // 코드는 싣지 않는다 — Host 의 오류 글에도 코드가 없다.
        setError(String(e instanceof Error ? e.message : e));
        console.warn(LOG, "remote.pairDirect failed");
      })
      .finally(() => setBusy(false));
  };
  return (
    <section
      aria-label={T.title}
      data-testid="remote-direct"
      className="flex flex-none flex-col gap-1 border-b border-ui-border px-3 py-2 text-[12px]"
    >
      <div className="flex items-center gap-2">
        <span className="flex-1 font-semibold">{T.title}</span>
        <button type="button" className={BUTTON} onClick={onClose}>
          {T.cancel}
        </button>
      </div>
      <p className="text-ui-muted-foreground">{T.hint}</p>
      <div className="flex items-center gap-2">
        <button type="button" className={BUTTON} disabled={searching} onClick={search}>
          {searching ? T.searching : T.search}
        </button>
        {notes.map((note) => (
          <span key={note} className="text-ui-muted-foreground">
            {note}
          </span>
        ))}
      </div>
      {found && found.length === 0 && <div className="text-ui-muted-foreground">{T.nothing}</div>}
      {found && found.length > 0 && (
        <ul aria-label={T.search} className="flex flex-col gap-0.5">
          {found.map((d) => (
            <li key={`${d.source}:${d.address}:${d.port}`}>
              <button
                type="button"
                data-discovered={d.address}
                className="flex w-full items-center gap-2 rounded-sm px-1 text-left hover:bg-ui-hover"
                onClick={() => pick(d)}
              >
                <span className="rounded-sm border border-ui-border px-1 text-[11px]">
                  {T.source[d.source]}
                </span>
                <span className="font-semibold">{d.label}</span>
                <span className="text-ui-muted-foreground">{d.address}</span>
                <span className={d.listening ? "text-ui-accent" : "text-ui-muted-foreground"}>
                  {d.listening ? T.listening : T.notListening}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-1">
          {T.address}
          <input
            className={FIELD}
            value={address}
            spellCheck={false}
            onChange={(e) => setAddress(e.currentTarget.value)}
          />
        </label>
        <label className="flex items-center gap-1">
          {T.port}
          <input
            className={`${FIELD} w-[70px]`}
            value={port}
            placeholder="7420"
            inputMode="numeric"
            onChange={(e) => setPort(e.currentTarget.value)}
          />
        </label>
        <label className="flex items-center gap-1">
          {T.nickname}
          <input
            className={FIELD}
            value={nickname}
            placeholder={suggestedName(address)}
            spellCheck={false}
            onChange={(e) => setNickname(e.currentTarget.value)}
          />
        </label>
        <label className="flex items-center gap-1">
          {T.code}
          <input
            className={`${FIELD} w-[90px] font-mono tracking-widest`}
            value={code}
            inputMode="numeric"
            autoComplete="one-time-code"
            onChange={(e) => setCode(e.currentTarget.value)}
          />
        </label>
        <button type="button" className={BUTTON} disabled={!ready || busy} onClick={pair}>
          {T.pair}
        </button>
      </div>
      {error && (
        <div role="alert" className="text-ui-accent">
          {error}
        </div>
      )}
      {done && <div role="status">{done}</div>}
    </section>
  );
}
