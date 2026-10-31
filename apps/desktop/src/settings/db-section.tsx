// 설정 창 「DB」(10-ui.md S7, 14-m3-plan.md M3-20): 전역 `settings.toml` 의 `[db]` — 그리드 보유 행 · 바이트 상한, 셀 상한, Redis 응답
// 타임아웃 · 키 구분자, 연결 타임아웃 기본값.
//
// - **Host 가 정본이다.** 범위 판정은 Host 의 `db.settings.set` 이 한다(crates/pw-db/src/settings.rs). 화면은 칸의 글을 수로 읽어 그대로
//   보내고, 거절의 까닭(`DbSettingRefused`)을 번역해 그 칸 아래에 보인다. 범위는 Host 가 알려 준 것을 안내로 적을 뿐 검사하지 않는다.
// - 파일의 값이 틀려 기본값을 쓰고 있으면(사람이 손으로 고친 범위 밖 값) 그렇다고 적는다. 「기본값으로」 는 그 키를 파일에서 지운다.
// - 바꾼 값은 다음 실행 · 다음 연결부터 쓰인다(이미 도는 실행 · 열린 연결은 그대로) — 칸 위에 적는다.

import {
  type DbSettingItem,
  type DbSettingKey,
  type DbSettingRefusal,
  type DbSettingsSetParams,
  type DbSettingsView,
  ERROR_CODE,
  RpcCallError,
} from "@pitwall/rpc-client";
import { useState } from "react";

import { useHost, useRpcQuery } from "../rpc/host";
import { type SettingsStrings, useSettingsStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";
const NOTE = "text-[12px] text-ui-muted-foreground";

type DbStrings = SettingsStrings["db"];

/** 칸의 글 → 보낼 값. 정수 칸은 수로 읽히면 수, 아니면 글 그대로 보낸다 — 판정은 Host 가 한다. */
export function inputValue(item: Pick<DbSettingItem, "unit">, text: string): unknown {
  if (item.unit === "text") return text;
  const trimmed = text.trim();
  const n = Number(trimmed);
  return trimmed !== "" && Number.isFinite(n) ? n : trimmed;
}

/** Host 가 거절한 까닭의 글. */
export function refusalText(reason: DbSettingRefusal, T: DbStrings): string {
  switch (reason.kind) {
    case "notInteger":
      return T.notInteger;
    case "range":
      return T.range(reason.min, reason.max);
    case "notText":
      return T.notText;
    case "separator":
      return T.separator(reason.max);
  }
}

/** `db.settings.set` 의 오류 글. 거절의 까닭이면 번역하고, 아니면(파일을 고칠 수 없음 등) Host 의 글. */
export function setErrorText(error: unknown, T: DbStrings): string {
  if (error instanceof RpcCallError && error.code === ERROR_CODE.INVALID_PARAMS) {
    const reason = (error.data as { reason?: DbSettingRefusal } | null | undefined)?.reason;
    if (reason && typeof reason === "object" && "kind" in reason) return refusalText(reason, T);
  }
  return T.failed(error instanceof Error ? error.message : String(error));
}

function shown(value: unknown): string {
  return typeof value === "number" || typeof value === "string" ? String(value) : "";
}

function unitText(item: DbSettingItem, T: DbStrings): string {
  switch (item.unit) {
    case "rows":
      return T.units.rows;
    case "bytes":
      return T.units.bytes;
    case "seconds":
      return T.units.seconds;
    case "text":
      return "";
  }
}

/** 바이트 값의 읽기 쉬운 글(KiB · MiB). */
export function bytesText(value: unknown): string | undefined {
  if (typeof value !== "number") return undefined;
  if (value % (1024 * 1024) === 0) return `${value / (1024 * 1024)} MiB`;
  if (value % 1024 === 0) return `${value / 1024} KiB`;
  return undefined;
}

export function DbSection() {
  const T = useSettingsStrings().db;
  const query = useRpcQuery("db.settings.get", null, { retry: false });
  const [latest, setLatest] = useState<DbSettingsView>();
  const view = latest ?? query.data;
  return (
    <section aria-labelledby="settings-db" className="flex flex-col gap-2">
      <h3 id="settings-db" className="font-semibold">
        {T.title}
      </h3>
      <span className={NOTE}>{T.intro}</span>
      {query.isPending && !view && <span className={NOTE}>{T.loading}</span>}
      {query.isError && !view && (
        <span role="alert" className="text-[12px] text-ui-accent">
          {T.failed(query.error instanceof Error ? query.error.message : String(query.error))}
        </span>
      )}
      {view && (
        <div className="grid grid-cols-[max-content_1fr] items-start gap-x-3 gap-y-2">
          {view.items.map((item) => (
            <SettingRow key={item.key} item={item} onSaved={setLatest} />
          ))}
        </div>
      )}
    </section>
  );
}

function SettingRow({
  item,
  onSaved,
}: {
  item: DbSettingItem;
  onSaved(view: DbSettingsView): void;
}) {
  const T = useSettingsStrings().db;
  const { client } = useHost();
  const [text, setText] = useState(shown(item.value));
  const [seen, setSeen] = useState(item.value);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  // Host 가 돌려준 값이 바뀌면(저장 · 기본값으로) 칸을 그 값으로 둔다.
  if (seen !== item.value) {
    setSeen(item.value);
    setText(shown(item.value));
  }
  const id = `db-setting-${item.key}`;
  const save = async (value: unknown) => {
    if (!client) return;
    setBusy(true);
    setError(undefined);
    try {
      onSaved(
        await client.call("db.settings.set", {
          key: item.key,
          value: value as DbSettingsSetParams["value"],
        }),
      );
    } catch (e) {
      setError(setErrorText(e, T));
    } finally {
      setBusy(false);
    }
  };
  const range =
    item.min !== null && item.max !== null
      ? T.rangeHint(item.min, item.max, unitText(item, T))
      : undefined;
  const readable = item.unit === "bytes" ? bytesText(item.value) : undefined;
  return (
    <>
      <label htmlFor={id} className="pt-0.5 text-ui-muted-foreground">
        {T.labels[item.key as DbSettingKey]}
      </label>
      <div data-setting={item.key} className="flex flex-col gap-0.5">
        <form
          className="flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void save(inputValue(item, text));
          }}
        >
          <input
            id={id}
            value={text}
            inputMode={item.unit === "text" ? "text" : "numeric"}
            onChange={(e) => setText(e.target.value)}
            aria-invalid={error !== undefined}
            className="w-36 rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
          />
          <span className={NOTE}>{unitText(item, T)}</span>
          <button type="submit" className={BUTTON} disabled={busy}>
            {T.apply}
          </button>
          <button
            type="button"
            className={BUTTON}
            disabled={busy || !item.configured}
            onClick={() => void save(null)}
          >
            {T.reset}
          </button>
        </form>
        <span className={NOTE}>
          {[T.defaultIs(shown(item.default)), range, readable && T.now(readable)]
            .filter(Boolean)
            .join(" · ")}
        </span>
        {item.refused && (
          <span data-refused="" className="text-[12px] text-ui-accent">
            {T.fileRefused(item.fileKey, refusalText(item.refused, T))}
          </span>
        )}
        {error && (
          <span role="alert" className="text-[12px] text-ui-accent">
            {error}
          </span>
        )}
      </div>
    </>
  );
}
