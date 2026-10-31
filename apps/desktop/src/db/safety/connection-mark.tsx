// 연결 표지(14-m3-plan.md M3-19 ① ④, 10-ui.md 「주요 툴 윈도우 상세」 의 DB 그림 아래): 연결의 색 태그와 라벨, 연결 결과의 TLS 표지.
//
// - 색은 연결 설정의 `color`(테마 토큰 `db.tag.*` 이름 또는 `#rrggbb`)다. **프로덕션 연결은 `color` 와 상관없이 `db.production`
//   (빨강)** — 프로덕션을 다른 색으로 칠해 덜 위험해 보이게 할 수 없다. 그 밖의 값(모르는 이름 · CSS 식)은 버린다 — 설정 파일은
//   커밋된 남의 글이라 `style` 에 그대로 넣지 않는다.
// - 색만으로 가르지 않는다(10-ui.md 「접근성」): 프로덕션 · 읽기 전용은 늘 글자 라벨이 함께 나온다.
// - 탭 · 콘솔 테두리는 `connectionFrameStyle` 을 그 요소의 `style` 에 펼친다(M3-15 · M3-17 · M3-18). 상태바는 db-status.tsx.

import type { TlsStatus } from "@pitwall/rpc-client";
import { ShieldAlert, ShieldOff } from "lucide-react";
import type { CSSProperties } from "react";

import { DB_TAG_TOKENS, tokenCssVar } from "../../theme/tokens";
import { type DbSafetyStrings, useDbSafetyStrings } from "./strings";

/** 표지가 보는 연결의 몫(`DbConnectionConfig` · `DbSessionOpened` 의 같은 이름 필드). */
export interface DbConnectionMarkInput {
  production?: boolean | null;
  readOnly?: boolean | null;
  color?: string | null;
}

const HEX = /^#[0-9a-fA-F]{6}$/;
const TAGS: ReadonlySet<string> = new Set(DB_TAG_TOKENS);

/**
 * 연결의 표지 색(CSS 값 — `var(--pw-…)` 또는 `#rrggbb`). 칠할 색이 없으면 `undefined`. 프로덕션은 늘 `db.production` 이다.
 */
export function connectionColor(input: DbConnectionMarkInput): string | undefined {
  if (input.production) return `var(${tokenCssVar("db.production")})`;
  const color = input.color?.trim();
  if (!color) return undefined;
  if (HEX.test(color)) return color;
  if (TAGS.has(color)) return `var(${tokenCssVar(color)})`;
  return undefined;
}

/** 연결의 글자 라벨(색과 함께 보인다). 순서는 프로덕션 → 읽기 전용. */
export function connectionLabels(
  input: DbConnectionMarkInput,
  T: DbSafetyStrings["label"],
): { kind: "production" | "readOnly"; text: string }[] {
  const labels: { kind: "production" | "readOnly"; text: string }[] = [];
  if (input.production) labels.push({ kind: "production", text: T.production });
  if (input.readOnly) labels.push({ kind: "readOnly", text: T.readOnly });
  return labels;
}

/**
 * 탭 · 콘솔의 테두리. 색이 없으면 `undefined` — 테두리를 바꾸지 않는다. 프로덕션은 두껍게(2px) 둔다.
 * `border` 를 이미 그리는 요소에 펼친다(`style={{ ...connectionFrameStyle(c) }}`).
 */
export function connectionFrameStyle(input: DbConnectionMarkInput): CSSProperties | undefined {
  const color = connectionColor(input);
  if (!color) return undefined;
  return { borderColor: color, borderWidth: input.production ? 2 : 1, borderStyle: "solid" };
}

const LABEL_CLASS: Record<"production" | "readOnly", string> = {
  production: "border-db-production text-db-production",
  readOnly: "border-db-read-only text-db-read-only",
};

/**
 * 연결 표지: 색 점 · 이름(있으면) · 라벨(「프로덕션」 · 「읽기 전용」). 탐색기 행 · 콘솔 위 막대 · 확인 다이얼로그 · 상태바가 쓴다.
 */
export function ConnectionMark({
  name,
  ...input
}: DbConnectionMarkInput & {
  /** 연결 이름. 없으면 점과 라벨만. */
  name?: string;
}) {
  const T = useDbSafetyStrings().label;
  const color = connectionColor(input);
  return (
    <span
      data-testid="db-connection-mark"
      data-production={input.production ? "true" : undefined}
      data-read-only={input.readOnly ? "true" : undefined}
      className="inline-flex min-w-0 items-center gap-1"
    >
      {color && (
        <span
          aria-hidden
          data-testid="db-connection-color"
          className="inline-block size-2 flex-none rounded-full"
          style={{ backgroundColor: color }}
        />
      )}
      {name !== undefined && <span className="truncate">{name}</span>}
      {connectionLabels(input, T).map((label) => (
        <span
          key={label.kind}
          data-label={label.kind}
          className={`flex-none rounded-sm border px-1 text-[10px] leading-[14px] font-semibold ${LABEL_CLASS[label.kind]}`}
        >
          {label.text}
        </span>
      ))}
    </span>
  );
}

/** TLS 상태의 표지. 검증한 TLS · 파일 DB 는 그리지 않는다(`null`). */
export function describeTls(
  tls: TlsStatus,
  T: DbSafetyStrings["tls"],
): { kind: "unverified" | "plaintext"; text: string; hint: string } | undefined {
  switch (tls) {
    case "unverified":
      return { kind: "unverified", text: T.unverified, hint: T.unverifiedHint };
    case "plaintext":
      return { kind: "plaintext", text: T.plaintext, hint: T.plaintextHint };
    case "verified":
    case "notApplicable":
      return undefined;
  }
}

/** 「서버 인증서를 검증하지 않음」 · 「평문」 표지(연결 결과 `ServerInfo.tls`). 검증한 TLS · SQLite 는 아무것도 그리지 않는다. */
export function TlsMark({ tls }: { tls: TlsStatus }) {
  const T = useDbSafetyStrings().tls;
  const view = describeTls(tls, T);
  if (!view) return null;
  const Icon = view.kind === "plaintext" ? ShieldOff : ShieldAlert;
  return (
    <span
      data-testid="db-tls-mark"
      data-tls={view.kind}
      title={view.hint}
      className="inline-flex items-center gap-1 text-db-insecure"
    >
      <Icon size={12} aria-hidden />
      {view.text}
    </span>
  );
}
