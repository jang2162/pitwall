// 연결 편집 다이얼로그의 폼 모델(M3-14): 칸은 생성물 schema.json 에서 읽고 스키마의 칸은 모두 정확히 한 탭에 있다(Host 가 칸을
// 더하면 여기서 실패해 자리를 정하게 한다), sslmode 의 처음 값 규칙(결정 M3-D10 — Host 의 `default_ssl_mode` 와 같은 예),
// 드라이버마다의 칸, 폼 → 설정, 비밀의 대상(Host 의 `DbSecretTarget::of`), 이름 → id.

import type { DbConnectionConfig, SslMode } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  type ConfigKey,
  configOf,
  defaultSslMode,
  fieldApplies,
  fieldsFromSchema,
  formOf,
  idFromName,
  loadUpsertSchema,
  newForm,
  secretTarget,
  TABS,
  tabsFor,
} from "./form";

const ALL_SSL: SslMode[] = ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"];

describe("fields from the generated schema", () => {
  it("place every config field of the schema in exactly one tab", async () => {
    const fields = fieldsFromSchema(await loadUpsertSchema());
    const placed = Object.values(TABS).flat();
    expect([...placed].sort()).toEqual([...fields.keys()].sort());
    expect(new Set(placed).size).toBe(placed.length);
  });

  it("read kinds, enums, bounds and required keys from the schema", async () => {
    const fields = fieldsFromSchema(await loadUpsertSchema());
    expect(fields.get("driver")).toMatchObject({
      kind: "enum",
      values: ["postgres", "mysql", "sqlite", "redis"],
      required: true,
    });
    expect(fields.get("sslmode")).toMatchObject({ kind: "enum", values: ALL_SSL, required: false });
    expect(fields.get("port")).toMatchObject({ kind: "integer", maximum: 65535 });
    expect(fields.get("readOnly")?.kind).toBe("boolean");
    expect(fields.get("name")).toMatchObject({ kind: "text", required: true });
    expect(fields.get("connectTimeout")?.kind).toBe("integer");
    expect(fields.get("password")?.kind).toBe("object");
  });
});

describe("sslmode default (decision M3-D10)", () => {
  it.each([
    ["", "verify-full"],
    ["db.example.com", "verify-full"],
    ["db.internal", "verify-full"],
    ["10.0.0.5", "verify-full"],
    ["localhost", "disable"],
    ["LocalHost", "disable"],
    ["127.0.0.1", "disable"],
    ["::1", "disable"],
    ["[::1]", "disable"],
    ["127.0.0.2", "verify-full"],
    ["localhost.example.com", "verify-full"],
  ])("%s → %s", (host, mode) => {
    expect(defaultSslMode(host)).toBe(mode);
  });

  it("a new connection starts at verify-full", () => {
    expect(newForm("x").sslmode).toBe("verify-full");
  });
});

describe("fields per driver", () => {
  it("sqlite has only the file and the options; the others have no path", () => {
    const keys = Object.values(TABS).flat() as ConfigKey[];
    expect(keys.filter((k) => fieldApplies("sqlite", k)).sort()).toEqual(
      ["color", "connectTimeout", "driver", "name", "path", "production", "readOnly"].sort(),
    );
    for (const driver of ["postgres", "mysql", "redis"] as const) {
      expect(fieldApplies(driver, "path")).toBe(false);
      expect(fieldApplies(driver, "sshTunnel")).toBe(true);
    }
    expect(tabsFor("sqlite")).toEqual(["general", "options"]);
    expect(tabsFor("postgres")).toEqual(["general", "ssh", "ssl", "options"]);
  });
});

describe("form → config", () => {
  const form = {
    ...newForm("prod"),
    name: " prod pg ",
    host: " db.internal ",
    port: "6432",
    database: "billing",
    user: "app",
    tunnel: true,
    hostRef: "bastion",
    sslmode: "verify-full" as const,
    sslrootcert: "certs/ca.pem",
    production: true,
    color: "db.tag.blue",
    connectTimeout: "30",
    path: "ignored.db",
  };

  it("keeps the fields of the driver and drops the others", () => {
    expect(configOf(form)).toEqual({
      name: "prod pg",
      driver: "postgres",
      host: "db.internal",
      port: 6432,
      database: "billing",
      user: "app",
      password: { kind: "keychain" },
      readOnly: false,
      production: true,
      color: "db.tag.blue",
      sslmode: "verify-full",
      sslrootcert: "certs/ca.pem",
      sslcert: null,
      sslkey: null,
      sshTunnel: { hostRef: "bastion" },
      path: null,
      connectTimeout: 30,
    } satisfies DbConnectionConfig);
    const sqlite = configOf({ ...form, driver: "sqlite", path: "data/app.db" });
    expect(sqlite).toMatchObject({
      host: null,
      port: null,
      password: null,
      sshTunnel: null,
      sslmode: null,
      path: "data/app.db",
    });
  });

  it("leaves the sslmode to the host when the person did not choose one", () => {
    expect(configOf(form, { keepAutomaticSslMode: true }).sslmode).toBeNull();
  });

  it("passes a non-number as it is so that the host refuses it", () => {
    expect(configOf({ ...form, connectTimeout: "abc" }).connectTimeout).toBe("abc");
    expect(configOf({ ...form, port: "" }).port).toBeNull();
  });

  it("sends env references by name and no password for none", () => {
    expect(
      configOf({ ...form, passwordSource: "env", envVariable: " PGPASSWORD " }).password,
    ).toEqual({
      kind: "env",
      variable: "PGPASSWORD",
    });
    expect(configOf({ ...form, passwordSource: "none" }).password).toBeNull();
  });

  it("round-trips a saved connection, picking the personal file when it exists", () => {
    const config = configOf(form);
    const back = formOf({
      id: "prod",
      config,
      layers: ["shared", "local"],
      readOnly: false,
      state: { kind: "ready" },
    });
    expect(configOf(back)).toEqual(config);
    expect(back.location).toBe("local");
    expect(
      formOf({ id: "a", config, layers: ["shared"], readOnly: false, state: { kind: "ready" } })
        .location,
    ).toBe("shared");
  });
});

describe("secret target (Host DbSecretTarget::of)", () => {
  it("fills the default port and the default sslmode", () => {
    expect(
      secretTarget({
        name: "x",
        driver: "mysql",
        host: "localhost",
        user: "root",
        sshTunnel: { hostRef: "b" },
      }),
    ).toEqual({
      driver: "mysql",
      host: "localhost",
      port: 3306,
      user: "root",
      tunnel: "b",
      sslmode: "disable",
      sslrootcert: null,
      sslcert: null,
      sslkey: null,
    });
    expect(
      secretTarget({ name: "x", driver: "postgres", host: "db", port: 6432, sslmode: "require" }),
    ).toMatchObject({
      port: 6432,
      sslmode: "require",
    });
  });
});

describe("connection id from the name", () => {
  it("keeps the host's characters and avoids taken ids", () => {
    expect(idFromName("Prod PG (billing)", [])).toBe("prod-pg-billing");
    expect(idFromName("운영 DB", [])).toBe("db");
    expect(idFromName("", [])).toBe("connection");
    expect(idFromName("dev", ["DEV", "dev-2"])).toBe("dev-3");
    expect(idFromName("x".repeat(100), []).length).toBeLessThanOrEqual(64);
  });
});
