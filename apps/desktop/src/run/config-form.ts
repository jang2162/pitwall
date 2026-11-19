// 실행 구성 편집 폼의 모델(17-m6-plan M6-07 ③): 칸은 생성물 schema.json 의 `RunConfig`(M6-01 의 `schemars` 출력 — MCP 검증과 같은
// 것)에서 읽는다(10-ui.md 「UI 스택」 의 폼 행 — 칸 · 타입 · 필수 · enum 은 스키마가 정본, db/connection/form.ts 와 같은 꼴). 종류
// (`type`)마다의 칸은 스키마의 `oneOf` 갈래에서, 공통 칸은 `RunConfig` 의 `properties` 에서 온다. 여기 표로 적는 것은 폼이 직접
// 그리는 공통 칸(이름 · 작업 폴더 · env · `.env` 파일)과 폼 밖에 두는 칸(프로필 · `[debug]` — 그대로 둔다)뿐이다.
//
// - 판정(id 꼴 · 루트 밖 `cwd` · 쓸 수 없는 값)은 Host 가 한다 — 저장이 거절하면 그 글을 보인다. 화면은 필수 칸이 비었는지만 막는다.
// - 주석 · 모르는 키는 Host 의 쓰기(`pw_run::config` 의 `plan` → `apply`)가 지킨다. 폼은 그 구성의 값만 보낸다.

import type { RunConfig, RunConfigEntry } from "@pitwall/rpc-client";

import type { JsonSchemaNode } from "../db/connection/form";

/** 칸의 꼴: 글 한 줄 · 글 목록(한 줄에 하나) · 고르기. */
export type RunFieldKind = "text" | "list" | "enum";

export interface RunFieldSpec {
  key: string;
  kind: RunFieldKind;
  /** `enum` 의 값(스키마 순서). */
  values: string[];
  required: boolean;
  /** `null` 이 될 수 있다(비우면 `null` 로 보낸다). */
  nullable: boolean;
  description?: string;
}

/** 종류 하나. */
export interface RunKindSpec {
  type: string;
  description?: string;
  fields: RunFieldSpec[];
}

export interface RunFormSchema {
  kinds: RunKindSpec[];
  /** `RunConfig` 의 공통 칸 중 필수인 것(`name`). */
  commonRequired: string[];
}

/** 폼이 직접 그리는 공통 칸. 스키마의 공통 칸은 모두 이것이거나 [`KEPT`] 다(config-form.test.ts). */
export const COMMON = ["name", "cwd", "env", "envFile", "before"] as const;
/** 폼 밖에 두고 그대로 보내는 공통 칸. */
export const KEPT = ["profiles", "debug"] as const;

function resolve(node: JsonSchemaNode, defs: Record<string, JsonSchemaNode>): JsonSchemaNode {
  if (node.$ref) {
    const target = defs[node.$ref.replace("#/$defs/", "")];
    if (!target) throw new Error(`schema: unknown $ref ${node.$ref}`);
    return { ...resolve(target, defs), description: node.description ?? target.description };
  }
  const some = node.anyOf?.filter((n) => n.type !== "null");
  if (some && some.length === 1)
    return { ...resolve(some[0]!, defs), description: node.description };
  return node;
}

function fieldOf(
  key: string,
  raw: JsonSchemaNode,
  required: boolean,
  defs: Record<string, JsonSchemaNode>,
): RunFieldSpec {
  const node = resolve(raw, defs) as JsonSchemaNode & { items?: JsonSchemaNode };
  const types = Array.isArray(node.type) ? node.type : [node.type];
  const nullable = types.includes("null") || (raw.anyOf?.some((n) => n.type === "null") ?? false);
  const description = raw.description ?? node.description;
  const base = { key, required, nullable, ...(description !== undefined && { description }) };
  if (node.enum) return { ...base, kind: "enum", values: node.enum };
  if (node.oneOf?.every((n) => typeof n.const === "string"))
    return { ...base, kind: "enum", values: node.oneOf.map((n) => n.const!) };
  if (types.includes("array")) return { ...base, kind: "list", values: [] };
  return { ...base, kind: "text", values: [] };
}

/** `run.config.save` 의 입력 스키마(`schema.json` 의 `methods`)에서 종류마다의 칸. */
export function runFormSchema(saveParams: JsonSchemaNode): RunFormSchema {
  const defs = saveParams.$defs ?? {};
  const config = defs.RunConfig as (JsonSchemaNode & { oneOf?: JsonSchemaNode[] }) | undefined;
  if (!config?.oneOf) throw new Error("schema: RunConfig has no kinds");
  const kinds = config.oneOf.map((variant): RunKindSpec => {
    const type = variant.properties?.type?.const;
    if (typeof type !== "string") throw new Error("schema: a RunConfig kind has no type");
    const required = new Set(variant.required ?? []);
    const fields = Object.entries(variant.properties ?? {})
      .filter(([key]) => key !== "type")
      .map(([key, raw]) => fieldOf(key, raw, required.has(key), defs));
    return {
      type,
      fields,
      ...(variant.description !== undefined && { description: variant.description }),
    };
  });
  return { kinds, commonRequired: config.required ?? [] };
}

/** 생성물 schema.json(큰 파일 — 편집기를 열 때만 읽는다)에서 `run.config.save` 의 입력 스키마. */
export async function loadSaveSchema(): Promise<JsonSchemaNode> {
  const schema = (await import("@pitwall/rpc-client/src/generated/schema.json")) as unknown as {
    methods: Record<string, { params: JsonSchemaNode }>;
  };
  const method = schema.methods["run.config.save"];
  if (!method) throw new Error("schema: run.config.save is missing");
  return method.params;
}

export type RunConfigLocation = "shared" | "local";

/** 폼의 값. 글 칸은 모두 문자열이다(빈 칸 = 없음, 목록 칸은 한 줄에 하나). */
export interface RunConfigForm {
  id: string;
  name: string;
  type: string;
  cwd: string;
  env: { name: string; value: string }[];
  envFile: string;
  /** 먼저 끝까지 돌릴 구성 id(한 줄에 하나 — 17-m6-plan M6-21). */
  before: string;
  /** 종류의 칸(`RunKindSpec.fields` 의 `key`). */
  kind: Record<string, string>;
  location: RunConfigLocation;
}

export const NEW_TYPE = "shell";

function kindValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(String).join("\n");
  if (value === null || value === undefined) return "";
  return String(value);
}

/** 구성 → 폼. `location` 은 처음 고를 저장 위치. */
export function formOf(
  id: string,
  config: RunConfig,
  location: RunConfigLocation = "shared",
): RunConfigForm {
  const record = config as unknown as Record<string, unknown>;
  const kind: Record<string, string> = {};
  for (const [key, value] of Object.entries(record)) {
    if (key === "type" || (COMMON as readonly string[]).includes(key)) continue;
    if ((KEPT as readonly string[]).includes(key)) continue;
    kind[key] = kindValue(value);
  }
  return {
    id,
    name: config.name,
    type: config.type,
    cwd: config.cwd ?? "",
    env: Object.entries(config.env ?? {}).map(([name, value]) => ({ name, value })),
    envFile: (config.envFile ?? []).join("\n"),
    before: (config.before ?? []).join("\n"),
    kind,
    location,
  };
}

/** 저장한 구성의 폼. 처음 고를 위치는 값을 준 파일 중 마지막(개인 파일이 있으면 그것 — Host 가 공유로 보인 파일은 공유). */
export function formOfEntry(entry: RunConfigEntry): RunConfigForm {
  return formOf(
    entry.id,
    entry.config,
    entry.layers.some((l) => l.origin === "local") ? "local" : "shared",
  );
}

export function newForm(id: string): RunConfigForm {
  return {
    id,
    name: "",
    type: NEW_TYPE,
    cwd: "",
    env: [],
    envFile: "",
    before: "",
    kind: {},
    location: "shared",
  };
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");

/**
 * 폼 → 구성. 고른 종류의 칸만 보낸다(종류를 바꾸면 앞 종류의 칸은 빠진다). `kept` 는 편집을 시작한 구성 — 폼 밖의 칸(프로필 ·
 * `[debug]`)을 그대로 넘긴다.
 */
export function configOf(form: RunConfigForm, schema: RunFormSchema, kept?: RunConfig): RunConfig {
  const spec = schema.kinds.find((k) => k.type === form.type);
  const out: Record<string, unknown> = {
    name: form.name.trim(),
    type: form.type,
    cwd: form.cwd.trim() === "" ? null : form.cwd.trim(),
    env: Object.fromEntries(
      form.env.filter((v) => v.name.trim() !== "").map((v) => [v.name.trim(), v.value]),
    ),
    envFile: lines(form.envFile),
    before: lines(form.before),
  };
  for (const key of KEPT) {
    const value = (kept as unknown as Record<string, unknown> | undefined)?.[key];
    if (value !== undefined) out[key] = value;
  }
  for (const field of spec?.fields ?? []) {
    const raw = form.kind[field.key] ?? "";
    if (field.kind === "list") out[field.key] = lines(raw);
    else if (raw.trim() === "") {
      if (field.nullable) out[field.key] = null;
    } else out[field.key] = raw;
  }
  return out as unknown as RunConfig;
}

/** 비어 있는 필수 칸(공통 칸 `name` 포함)의 key. 목록 칸은 한 줄 이상이어야 한다. */
export function missingRequired(form: RunConfigForm, schema: RunFormSchema): string[] {
  const missing: string[] = [];
  if (form.id.trim() === "") missing.push("id");
  if (schema.commonRequired.includes("name") && form.name.trim() === "") missing.push("name");
  const spec = schema.kinds.find((k) => k.type === form.type);
  for (const field of spec?.fields ?? []) {
    if (!field.required) continue;
    const raw = form.kind[field.key] ?? "";
    if (field.kind === "list" ? lines(raw).length === 0 : raw.trim() === "")
      missing.push(field.key);
  }
  return missing;
}

/** 이름에서 구성 id(파일 이름 — Host 규칙: 영문자 · 숫자 · `-` · `_`, 64 글자까지). 겹치면 `-2` … */
export function idFromName(name: string, taken: readonly string[]): string {
  const base =
    name
      .normalize("NFKD")
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 56) || "run";
  const used = new Set(taken.map((t) => t.toLowerCase()));
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) {
    const id = `${base}-${n}`;
    if (!used.has(id)) return id;
  }
}
