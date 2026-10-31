// 명령 콘솔(14-m3-plan M3-18): 입력 줄 하나 + 결과 기록. 보낸 글은 Host 의 토크나이저(`db.kv.parseCommand` — 결정: 콘솔 글을
// argv 로 나누는 것도 Host 하나가 한다, M3-22 ⓔ)로 나눈 뒤 `db.kv.command` 로 보낸다. 등급은 Host 가 정한다: `Confirm` 은 `3001`
// 을 M3-19 다이얼로그로 물어 다시 보내고, `Blocked` 는 거절 문장(`data.block`)을, read-only · 미신뢰 · 다른 창은 M3-19 의 거절 문장을
// 기록에 남긴다. `\xHH` 로 만든 UTF-8 이 아닌 인자는 `db.kv.command` 가 받지 못해 보내지 않고 그렇게 적는다.
//
// 기록은 최근 `MAX_LOG` 개 — 응답 하나의 줄은 reply-text.ts 가 묶는다. 입력 칸의 ↑ ↓ 는 보낸 명령을 다시 꺼낸다.

import type { DbKvConfirmRequired, KvReply, RpcClient } from "@pitwall/rpc-client";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { isComposingKey } from "../../ui/ime";
import { dbRefusal, dbRefusalText } from "../safety/messages";
import type { RiskConfirmAnswer } from "../safety/risk-confirm";
import { useDbSafetyStrings } from "../safety/strings";
import { errorText, kvBlockedOf, kvDbErrorOf, withKvConfirm } from "./kv-gate";
import { replyLines } from "./reply-text";
import { useDbRedisStrings } from "./strings";

/** 기록에 남기는 명령 수. */
export const MAX_LOG = 200;
/** ↑ ↓ 로 꺼내는 지난 명령 수. */
const MAX_HISTORY = 100;

type Outcome =
  | { kind: "reply"; lines: string[] }
  | { kind: "refusal"; text: string; reason: string }
  | { kind: "note"; text: string }
  | { kind: "error"; text: string };

interface LogEntry {
  id: number;
  command: string;
  outcome?: Outcome;
}

let nextId = 1;

export interface KvConsoleSession {
  client: RpcClient;
  sessionId: string;
  ask(request: DbKvConfirmRequired): Promise<RiskConfirmAnswer>;
  /** 명령에 답이 왔다 — 키를 바꿨을 수 있으니 값 뷰어가 다시 읽는다. */
  onWrite?(): void;
}

export function KvConsole({ session }: { session: KvConsoleSession | undefined }) {
  const T = useDbRedisStrings().console;
  const S = useDbSafetyStrings();
  const [log, setLog] = useState<LogEntry[]>([]);
  const [text, setText] = useState("");
  const history = useRef<string[]>([]);
  const back = useRef(-1);
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => end.current?.scrollIntoView?.({ block: "end" }), [log]);

  const settle = (id: number, outcome: Outcome) =>
    setLog((l) => l.map((e) => (e.id === id ? { ...e, outcome } : e)));

  const send = async () => {
    const command = text.trim();
    if (command === "") return;
    setText("");
    history.current = [command, ...history.current.filter((h) => h !== command)].slice(
      0,
      MAX_HISTORY,
    );
    back.current = -1;
    const id = nextId++;
    setLog((l) => [...l, { id, command }].slice(-MAX_LOG));
    if (!session) return settle(id, { kind: "error", text: T.needsSession });
    const { client, sessionId } = session;
    let argv: string[];
    try {
      const parsed = await client.call("db.kv.parseCommand", { text: command });
      if (parsed.argv.some((a) => typeof a !== "string"))
        return settle(id, { kind: "error", text: T.bytesArg });
      argv = parsed.argv as string[];
    } catch (e) {
      return settle(id, { kind: "error", text: T.parseFailed(errorText(e)) });
    }
    try {
      const result = await withKvConfirm<KvReply>(
        (confirmed) =>
          client.call("db.kv.command", { sessionId, argv, confirmed: confirmed ?? null }),
        session.ask,
      );
      if (result.kind === "cancelled") return settle(id, { kind: "note", text: T.cancelled });
      if (result.kind === "busy") return settle(id, { kind: "note", text: T.busy });
      settle(id, { kind: "reply", lines: replyLines(result.value, T.reply) });
      // 명령이 키를 바꿨을 수 있다 — 등급을 화면에서 다시 매기지 않으므로(분류는 Host 하나) 응답이 오면 늘 알린다.
      session.onWrite?.();
    } catch (e) {
      const blocked = kvBlockedOf(e);
      if (blocked)
        return settle(id, {
          kind: "refusal",
          reason: blocked.block,
          text: T.blocked[blocked.block](blocked.command || (argv[0] ?? "")),
        });
      const refusal = dbRefusal(e);
      if (refusal)
        return settle(id, {
          kind: "refusal",
          reason: refusal.reason,
          text: dbRefusalText(refusal, S.refusal),
        });
      const db = kvDbErrorOf(e);
      if (db?.code === "KEY_TOO_LARGE")
        return settle(id, { kind: "error", text: T.keyTooLarge(db.message) });
      settle(id, { kind: "error", text: T.failed(errorText(e)) });
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (isComposingKey(e.nativeEvent)) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void send();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      const h = history.current;
      if (h.length === 0) return;
      e.preventDefault();
      back.current =
        e.key === "ArrowUp"
          ? Math.min(h.length - 1, back.current + 1)
          : Math.max(-1, back.current - 1);
      setText(back.current < 0 ? "" : h[back.current]!);
    }
  };

  return (
    <section
      aria-label={T.label}
      data-testid="redis-console"
      className="flex min-h-0 flex-col border-t border-ui-border"
    >
      <div
        role="log"
        data-testid="redis-console-log"
        className="min-h-0 flex-1 overflow-auto px-2 py-1 font-mono text-[12px] select-text"
      >
        {log.map((entry) => (
          <div key={entry.id} data-testid="redis-console-entry" className="pb-1">
            <div className="text-ui-muted-foreground">&gt; {entry.command}</div>
            {entry.outcome?.kind === "reply" && (
              <pre className="whitespace-pre-wrap break-all">{entry.outcome.lines.join("\n")}</pre>
            )}
            {entry.outcome?.kind === "refusal" && (
              <p
                role="alert"
                data-testid="redis-console-refusal"
                data-reason={entry.outcome.reason}
                className="text-ui-accent"
              >
                {entry.outcome.text}
              </p>
            )}
            {entry.outcome?.kind === "note" && (
              <p className="text-ui-muted-foreground">{entry.outcome.text}</p>
            )}
            {entry.outcome?.kind === "error" && (
              <p role="alert" data-testid="redis-console-error" className="text-ui-accent">
                {entry.outcome.text}
              </p>
            )}
          </div>
        ))}
        <div ref={end} />
      </div>
      <div className="flex items-center gap-2 border-t border-ui-border px-2 py-1">
        <span aria-hidden className="font-mono text-ui-muted-foreground">
          &gt;
        </span>
        <input
          data-testid="redis-console-input"
          aria-label={T.label}
          placeholder={session ? T.placeholder : T.needsSession}
          value={text}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          className="min-w-0 flex-1 bg-transparent font-mono outline-none select-text"
        />
        <button type="button" data-action="send" disabled={text.trim() === ""} onClick={send}>
          {T.send}
        </button>
      </div>
    </section>
  );
}
