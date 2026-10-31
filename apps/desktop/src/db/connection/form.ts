// 연결 편집 다이얼로그(14-m3-plan.md M3-14)의 폼 모델: 칸 목록은 생성물 schema.json 의 `DbConnectionConfig` 스키마에서 읽고
// (10-ui.md 「UI 스택」 의 폼 행 — 칸 · 타입 · enum · 범위는 스키마가 정본), 어느 탭 · 어느 드라이버에 두는지만 여기 표로 적는다.
//
// - 판정은 Host 가 한다: 호스트 · 터널 목적지 글자, env 이름, 연결 타임아웃 범위, 연결 id 는 저장(`db.connection.upsert`)이
//   거절하면 그 글을 보인다. 화면은 빈 칸만 막는다.
// - `sslmode` 의 처음 값(결정 M3-D10): DB 호스트가 루프백(`localhost` · `127.0.0.1` · `::1`)이면 `disable`, 아니면 `verify-full`.
//   터널이 있어도 같은 규칙이다 — 호스트는 바스티언에서 본 이름이라 「바스티언 기준의 루프백」 이다. Host 의
//   `pw_db::config::default_ssl_mode` 와 같은 규칙이다(바꾸면 둘을 함께 고친다). 사람이 고르기 전까지 호스트를 따라 바뀐다.
// - 비밀번호 값은 이 모델에 없다 — 다이얼로그가 제어하지 않는 칸(ref)에서 저장 · 테스트 순간에만 읽는다.

import type {
  DbConfigLocation,
  DbConnectionConfig,
  DbConnectionEntry,
  DbSecretTarget,
  DriverKind,
  SslMode,
} from "@pitwall/rpc-client";

/** 생성물 schema.json 의 모양 중 여기서 읽는 몫. */
export interface JsonSchemaNode {
  type?: string | string[];
  enum?: string[];
  const?: string;
  oneOf?: JsonSchemaNode[];
  anyOf?: JsonSchemaNode[];
  $ref?: string;
  description?: string;
  minimum?: number;
  maximum?: number;
  properties?: Record<string, JsonSchemaNode>;
  required?: string[];
  $defs?: Record<string, JsonSchemaNode>;
}

export type ConfigKey = keyof DbConnectionConfig;

export type FieldKind = "text" | "integer" | "boolean" | "enum" | "object";

/** 스키마에서 읽은 칸 하나. */
export interface FieldSpec {
  key: ConfigKey;
  kind: FieldKind;
  /** `enum` 칸의 값(스키마 순서). */
  values: string[];
  required: boolean;
  minimum?: number;
  maximum?: number;
  description?: string;
}

function resolve(node: JsonSchemaNode, defs: Record<string, JsonSchemaNode>): JsonSchemaNode {
  if (node.$ref) {
    const name = node.$ref.replace("#/$defs/", "");
    const target = defs[name];
    if (!target) throw new Error(`schema: unknown $ref ${node.$ref}`);
    return resolve(target, defs);
  }
  // `Option<T>` 는 `anyOf: [T, null]` 다.
  const some = node.anyOf?.filter((n) => n.type !== "null");
  if (some && some.length === 1)
    return { ...resolve(some[0]!, defs), description: node.description };
  return node;
}

function kindOf(node: JsonSchemaNode): { kind: FieldKind; values: string[] } {
  if (node.enum) return { kind: "enum", values: node.enum };
  if (node.oneOf?.every((n) => typeof n.const === "string"))
    return { kind: "enum", values: node.oneOf.map((n) => n.const!) };
  const types = Array.isArray(node.type) ? node.type : [node.type];
  if (types.includes("boolean")) return { kind: "boolean", values: [] };
  if (types.includes("integer")) return { kind: "integer", values: [] };
  if (types.includes("string")) return { kind: "text", values: [] };
  return { kind: "object", values: [] };
}

/** `db.connection.upsert` 의 입력 스키마(`schema.json` 의 `methods`)에서 연결 설정의 칸들. */
export function fieldsFromSchema(upsertParams: JsonSchemaNode): Map<ConfigKey, FieldSpec> {
  const defs = upsertParams.$defs ?? {};
  const config = defs.DbConnectionConfig;
  if (!config?.properties) throw new Error("schema: DbConnectionConfig has no properties");
  const required = new Set(config.required ?? []);
  const fields = new Map<ConfigKey, FieldSpec>();
  for (const [key, raw] of Object.entries(config.properties)) {
    const node = resolve(raw, defs);
    const { kind, values } = kindOf(node);
    fields.set(key as ConfigKey, {
      key: key as ConfigKey,
      kind,
      values,
      required: required.has(key),
      ...(node.minimum !== undefined && { minimum: node.minimum }),
      ...(node.maximum !== undefined && { maximum: node.maximum }),
      ...((raw.description ?? node.description) !== undefined && {
        description: raw.description ?? node.description,
      }),
    });
  }
  return fields;
}

/** 생성물 schema.json(큰 파일 — 다이얼로그를 열 때만 읽는다)에서 `db.connection.upsert` 의 입력 스키마. */
export async function loadUpsertSchema(): Promise<JsonSchemaNode> {
  const schema = (await import("@pitwall/rpc-client/src/generated/schema.json")) as unknown as {
    methods: Record<string, { params: JsonSchemaNode }>;
  };
  const method = schema.methods["db.connection.upsert"];
  if (!method) throw new Error("schema: db.connection.upsert is missing");
  return method.params;
}

export type TabId = "general" | "ssh" | "ssl" | "options";

/**
 * 탭 배치. 스키마의 칸은 모두 정확히 한 탭에 있다(form.test.ts — Host 가 칸을 더하면 시험이 실패해 여기에 자리를 정하게 한다).
 * `password` · `sshTunnel` 은 전용 칸(출처 고르기 · 호스트 고르기)으로 그린다.
 */
export const TABS: Record<TabId, readonly ConfigKey[]> = {
  general: ["name", "driver", "host", "port", "database", "user", "password", "path"],
  ssh: ["sshTunnel"],
  ssl: ["sslmode", "sslrootcert", "sslcert", "sslkey"],
  options: ["readOnly", "production", "color", "connectTimeout"],
};

/** 파일 DB(SQLite)의 칸. 그 밖의 드라이버는 `path` 를 뺀 전부. */
const FILE_KEYS: ReadonlySet<ConfigKey> = new Set([
  "name",
  "driver",
  "path",
  "readOnly",
  "production",
  "color",
  "connectTimeout",
]);

/** 드라이버가 쓰는 칸인가(「드라이버를 바꾸면 그 드라이버의 칸만」). */
export function fieldApplies(driver: DriverKind, key: ConfigKey): boolean {
  return driver === "sqlite" ? FILE_KEYS.has(key) : key !== "path";
}

/** 그 드라이버에 보일 탭. SQLite 는 SSH · SSL 이 없다. */
export function tabsFor(driver: DriverKind): TabId[] {
  return (Object.keys(TABS) as TabId[]).filter((tab) =>
    TABS[tab].some((key) => fieldApplies(driver, key)),
  );
}

const LOOPBACK = new Set(["localhost", "127.0.0.1", "::1"]);

/** Host 의 `default_ssl_mode` 와 같은 규칙(머리 주석). */
export function defaultSslMode(host: string): SslMode {
  const bare = host.trim().replace(/^\[/, "").replace(/\]$/, "").toLowerCase();
  return LOOPBACK.has(bare) ? "disable" : "verify-full";
}

/** Host 의 `default_port` 와 같다. */
export function defaultPort(driver: DriverKind): number | null {
  switch (driver) {
    case "postgres":
      return 5432;
    case "mysql":
      return 3306;
    case "redis":
      return 6379;
    case "sqlite":
      return null;
  }
}

/** 검증하지 않는 TLS(칸 옆에 「서버 인증서를 검증하지 않는다」 를 적는다). */
export const UNVERIFIED_MODES: ReadonlySet<SslMode> = new Set(["allow", "prefer", "require"]);

export type PasswordSource = "none" | "keychain" | "env";

/** 폼의 값. 글 칸은 모두 문자열이다(빈 칸 = 없음). 비밀번호 값은 없다. */
export interface ConnectionForm {
  id: string;
  name: string;
  driver: DriverKind;
  host: string;
  port: string;
  database: string;
  user: string;
  passwordSource: PasswordSource;
  envVariable: string;
  path: string;
  tunnel: boolean;
  hostRef: string;
  sslmode: SslMode;
  sslrootcert: string;
  sslcert: string;
  sslkey: string;
  readOnly: boolean;
  production: boolean;
  color: string;
  connectTimeout: string;
  location: DbConfigLocation;
}

export const NEW_DRIVER: DriverKind = "postgres";

/** 「연결 추가」 의 처음 값. 비밀번호는 키체인(값은 그 칸에 친 것만 `db.secret.set` 으로). */
export function newForm(id: string): ConnectionForm {
  return {
    id,
    name: "",
    driver: NEW_DRIVER,
    host: "",
    port: "",
    database: "",
    user: "",
    passwordSource: "keychain",
    envVariable: "",
    path: "",
    tunnel: false,
    hostRef: "",
    sslmode: defaultSslMode(""),
    sslrootcert: "",
    sslcert: "",
    sslkey: "",
    readOnly: false,
    production: false,
    color: "",
    connectTimeout: "",
    location: "shared",
  };
}

/** 저장한 연결의 값. 처음 고를 저장 위치는 값을 준 파일 중 마지막(개인 파일이 있으면 그것). */
export function formOf(entry: DbConnectionEntry & { config: DbConnectionConfig }): ConnectionForm {
  const c = entry.config;
  const text = (v: string | null | undefined) => v ?? "";
  return {
    id: entry.id,
    name: c.name,
    driver: c.driver,
    host: text(c.host),
    port: c.port == null ? "" : String(c.port),
    database: text(c.database),
    user: text(c.user),
    passwordSource: c.password ? c.password.kind : "none",
    envVariable: c.password?.kind === "env" ? c.password.variable : "",
    path: text(c.path),
    tunnel: c.sshTunnel != null,
    hostRef: c.sshTunnel?.hostRef ?? "",
    sslmode: c.sslmode ?? defaultSslMode(text(c.host)),
    sslrootcert: text(c.sslrootcert),
    sslcert: text(c.sslcert),
    sslkey: text(c.sslkey),
    readOnly: c.readOnly ?? false,
    production: c.production ?? false,
    color: text(c.color),
    connectTimeout: c.connectTimeout == null ? "" : String(c.connectTimeout),
    location: entry.layers.includes("local") ? "local" : "shared",
  };
}

function optional(value: string): string | null {
  const v = value.trim();
  return v === "" ? null : v;
}

/** 숫자 칸. 숫자가 아니면 그 글 그대로 넘긴다 — 거절은 Host 의 몫이다(`NaN` 은 JSON 에서 `null` 이 되어 칸이 사라진다). */
function integer(value: string): number | null {
  const v = value.trim();
  if (v === "") return null;
  return /^\d+$/.test(v) ? Number(v) : (v as unknown as number);
}

/**
 * 폼 → 연결 설정. 드라이버가 쓰지 않는 칸은 넣지 않는다. `sslmode` 는 `keepAutomaticSslMode` 면 `null`(Host 가 호스트로 정한다
 * — 파일에 적지 않았던 연결을 사람이 건드리지 않았을 때), 아니면 고른 값을 적는다.
 */
export function configOf(
  form: ConnectionForm,
  { keepAutomaticSslMode = false }: { keepAutomaticSslMode?: boolean } = {},
): DbConnectionConfig {
  const network = form.driver !== "sqlite";
  const password =
    !network || form.passwordSource === "none"
      ? null
      : form.passwordSource === "keychain"
        ? { kind: "keychain" as const }
        : { kind: "env" as const, variable: form.envVariable.trim() };
  return {
    name: form.name.trim(),
    driver: form.driver,
    host: network ? optional(form.host) : null,
    port: network ? integer(form.port) : null,
    database: network ? optional(form.database) : null,
    user: network ? optional(form.user) : null,
    password,
    readOnly: form.readOnly,
    production: form.production,
    color: optional(form.color),
    sslmode: !network || keepAutomaticSslMode ? null : form.sslmode,
    sslrootcert: network ? optional(form.sslrootcert) : null,
    sslcert: network ? optional(form.sslcert) : null,
    sslkey: network ? optional(form.sslkey) : null,
    sshTunnel: network && form.tunnel ? { hostRef: form.hostRef.trim() } : null,
    path: network ? null : optional(form.path),
    connectTimeout: integer(form.connectTimeout),
  };
}

/**
 * 비밀의 대상(`db.secret.set` 의 `target`) — Host 의 `DbSecretTarget::of` 와 같다. 저장한 뒤 Host 가 돌려준 병합 설정
 * (`upsert` 의 답)으로 만든다 — 개인 파일에 쓰면 공유 파일에만 있던 키가 남기 때문이다. 다르면 Host 가 conflict 로 거절한다.
 */
export function secretTarget(config: DbConnectionConfig): DbSecretTarget {
  return {
    driver: config.driver,
    host: config.host ?? null,
    port: config.port ?? defaultPort(config.driver),
    user: config.user ?? null,
    tunnel: config.sshTunnel?.hostRef ?? null,
    sslmode: config.sslmode ?? (config.host ? defaultSslMode(config.host) : "disable"),
    sslrootcert: config.sslrootcert ?? null,
    sslcert: config.sslcert ?? null,
    sslkey: config.sslkey ?? null,
  };
}

/** 이름에서 연결 id(파일 이름 — Host 규칙: 영문자 · 숫자 · `-` · `_`, 64 글자까지). 이미 있는 id 와 겹치면 `-2` … 를 붙인다. */
export function idFromName(name: string, taken: readonly string[]): string {
  const base =
    name
      .normalize("NFKD")
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 56) || "connection";
  const used = new Set(taken.map((t) => t.toLowerCase()));
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) {
    const id = `${base}-${n}`;
    if (!used.has(id)) return id;
  }
}
