// 실행 구성 편집 폼의 모델(17-m6-plan M6-07 ③): 칸 · 필수는 생성물 schema.json 의 `RunConfig` 에서 온다(손으로 쓴 칸 목록이 아니다).
// 아래 스냅숏은 그 스키마에서 읽은 종류마다의 칸이다 — Host 가 종류 · 칸을 바꾸면 스냅숏이 바뀌어 사람이 본다.

import { describe, expect, it } from "vitest";

import schemaJson from "../../../../packages/rpc-client/src/generated/schema.json";
import type { JsonSchemaNode } from "../db/connection/form";
import {
  COMMON,
  configOf,
  formOf,
  KEPT,
  missingRequired,
  newForm,
  runFormSchema,
} from "./config-form";

const params = (schemaJson as unknown as { methods: Record<string, { params: JsonSchemaNode }> })
  .methods["run.config.save"]!.params;

describe("the run config form", () => {
  it("takes its kinds, fields and required fields from the schema", () => {
    const schema = runFormSchema(params);
    const summary = Object.fromEntries(
      schema.kinds.map((k) => [
        k.type,
        k.fields.map((f) => `${f.key}${f.required ? "*" : ""}:${f.kind}`),
      ]),
    );
    expect(summary).toMatchInlineSnapshot(`
      {
        "cargo": [
          "args:list",
          "command*:text",
        ],
        "docker-compose": [
          "file:text",
          "services:list",
        ],
        "go": [
          "args:list",
          "command*:text",
          "package:text",
        ],
        "gradle": [
          "tasks*:list",
        ],
        "maven": [
          "tasks*:list",
        ],
        "mise-task": [
          "task*:text",
        ],
        "npm": [
          "script*:text",
        ],
        "python": [
          "args:list",
          "module:text",
          "script:text",
        ],
        "shell": [
          "command*:text",
        ],
        "sql": [
          "connection*:text",
          "files*:list",
          "transaction:enum",
        ],
      }
    `);
    expect(schema.commonRequired).toEqual(["name"]);
  });

  it("follows the schema rather than a list written here", () => {
    // 스키마가 칸 하나를 필수로 더하면 폼도 그 칸을 필수로 본다 — 칸 목록을 손으로 적으면 이 시험이 실패한다.
    const changed = structuredClone(params) as JsonSchemaNode & {
      $defs: { RunConfig: { oneOf: JsonSchemaNode[] } };
    };
    const shell = changed.$defs.RunConfig.oneOf.find((v) => v.properties?.type?.const === "shell")!;
    shell.properties!.login = { type: "string" };
    shell.required = [...(shell.required ?? []), "login"];
    const schema = runFormSchema(changed);
    expect(schema.kinds.find((k) => k.type === "shell")!.fields.map((f) => f.key)).toContain(
      "login",
    );
    const form = { ...newForm("dev"), name: "Dev", kind: { command: "x" } };
    expect(missingRequired(form, schema)).toEqual(["login"]);
    expect(missingRequired(form, runFormSchema(params))).toEqual([]);
  });

  it("covers every common field of the schema, in the form or kept as it is", () => {
    const config = (params.$defs as Record<string, JsonSchemaNode>).RunConfig!;
    expect(Object.keys(config.properties ?? {}).sort()).toEqual([...COMMON, ...KEPT].sort());
  });

  it("sends only the chosen kind's fields and keeps profiles and debug", () => {
    const schema = runFormSchema(params);
    const kept = {
      name: "Dev",
      type: "cargo" as const,
      command: "run",
      args: ["--release"],
      profiles: { fast: { args: ["-q"] } },
      debug: { adapter: "lldb", mode: "launch" as const },
    };
    const form = formOf("dev", kept, "local");
    expect(form.kind).toEqual({ command: "run", args: "--release" });
    expect(configOf(form, schema, kept)).toEqual({
      name: "Dev",
      type: "cargo",
      cwd: null,
      env: {},
      envFile: [],
      command: "run",
      args: ["--release"],
      profiles: { fast: { args: ["-q"] } },
      debug: { adapter: "lldb", mode: "launch" },
    });
    // 종류를 바꾸면 앞 종류의 칸은 빠진다.
    const shell = configOf(
      { ...form, type: "shell", kind: { ...form.kind, command: "ls" } },
      schema,
      kept,
    );
    expect(shell).not.toHaveProperty("args");
    expect(shell).toMatchObject({ type: "shell", command: "ls" });
  });

  it("turns lines, empty values and env rows into the config shape", () => {
    const schema = runFormSchema(params);
    const config = configOf(
      {
        ...newForm("api"),
        name: " API ",
        type: "python",
        cwd: " app ",
        env: [
          { name: "PORT", value: "3000" },
          { name: " ", value: "ignored" },
        ],
        envFile: ".env\n\n .env.local \n",
        kind: { module: "app", script: "", args: "--reload\n" },
      },
      schema,
    );
    expect(config).toEqual({
      name: "API",
      type: "python",
      cwd: "app",
      env: { PORT: "3000" },
      envFile: [".env", ".env.local"],
      module: "app",
      script: null,
      args: ["--reload"],
    });
  });
});
