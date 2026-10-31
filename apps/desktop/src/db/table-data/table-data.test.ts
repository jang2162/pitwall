// 테이블 데이터의 순수 몫(14-m3-plan.md M3-17): 읽는 글 · ORDER BY · FK 의 WHERE(table-sql.ts), 변경 → `changes`(changes.ts),
// 편집 버퍼(edit-buffer.ts), 실패 문장(failures.ts), 탭의 꼴(documents/spec.ts), 액션 인자(table-data-feature.tsx).

import { ERROR_CODE, RpcCallError, type DbValue } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { bundledTranslator } from "../../i18n/testing";
import { isDbDocumentSpec } from "../documents/spec";
import { dbSafetyStrings } from "../safety/strings";
import {
  buildChanges,
  editableValue,
  keyColumnsOf,
  sameColumnName,
  sameValue,
  valueFromInput,
} from "./changes";
import { DEFAULT_CELL, EditBuffer } from "./edit-buffer";
import { editFailureText } from "./failures";
import { dbTableDataStrings } from "./strings";
import { openArgs } from "./table-data-feature";
import { foreignKeyWhere, orderByColumn, sqlLiteral, tableSelect } from "./table-sql";

const T = dbTableDataStrings(bundledTranslator("ko"));
const S = dbSafetyStrings(bundledTranslator("ko"));
const T_ = { database: "app", schema: "public", name: "t" };

const sync = (run: () => void) => run();

function loaded(rows: DbValue[][]): EditBuffer {
  const b = new EditBuffer(sync);
  b.setColumns([
    { name: "id", dbType: "int4", nullable: false },
    { name: "name", dbType: "text", nullable: true },
    { name: "data", dbType: "bytea", nullable: true },
  ]);
  b.append(rows);
  b.finish({ kind: "done", truncated: false, elapsedMs: 1, rowsAffected: null });
  return b;
}

describe("table-sql", () => {
  it("quotes the table always, keeps the fragments on their own lines and drops empty clauses", () => {
    expect(tableSelect("postgres", T_, { where: "", orderBy: " " })).toBe(
      'SELECT * FROM "public"."t"',
    );
    expect(
      tableSelect("postgres", { ...T_, schema: "Weird" }, { where: " a = 1 -- x", orderBy: "b" }),
    ).toBe('SELECT * FROM "Weird"."t"\nWHERE a = 1 -- x\nORDER BY b');
    expect(
      tableSelect(
        "mysql",
        { database: "shop", schema: null, name: "order" },
        { where: "", orderBy: "" },
      ),
    ).toBe("SELECT * FROM `shop`.`order`");
    expect(
      tableSelect(
        "sqlite",
        { database: "main", schema: null, name: 'we"ird' },
        { where: "1", orderBy: "" },
      ),
    ).toBe('SELECT * FROM "we""ird"\nWHERE 1');
  });

  it("orders by a column with the dialect's quoting only where needed", () => {
    expect(orderByColumn("postgres", "name", "asc")).toBe("name ASC");
    expect(orderByColumn("postgres", "Name", "desc")).toBe('"Name" DESC');
    expect(orderByColumn("mysql", "order", "asc")).toBe("`order` ASC");
    expect(orderByColumn("postgres", "name", undefined)).toBe("");
  });

  it("takes the literal from db.sql.insertText and ANDs the foreign key columns", async () => {
    const asked: unknown[] = [];
    const client = {
      call: async (_m: string, params: { rows: DbValue[][] }) => {
        asked.push(params);
        const v = params.rows[0]![0]!;
        const lit =
          v !== "null" && v.text !== undefined ? `E'${v.text.replaceAll("'", "''")}); x'` : "7";
        return { text: `INSERT INTO MY_TABLE ("v") VALUES (${lit});\n` };
      },
    } as never;
    // 리터럴 안에 `);` 가 있어도 끝의 `);` 까지가 리터럴이다.
    expect(await sqlLiteral(client, "postgres", { text: "a" })).toBe("E'a); x'");
    expect(
      await foreignKeyWhere(client, "postgres", [
        { column: "id", value: { int: 7 } },
        { column: "Code", value: { text: "o'k" } },
      ]),
    ).toBe(`id = 7 AND "Code" = E'o''k); x'`);
    expect(asked[0]).toEqual({
      dialect: "postgres",
      table: null,
      columns: ["v"],
      rows: [[{ text: "a" }]],
    });
    expect(
      await foreignKeyWhere(client, "postgres", [{ column: "id", value: "null" }]),
    ).toBeUndefined();
  });
});

describe("changes", () => {
  it("matches key columns by the dialect's rule and refuses a missing or doubled one", () => {
    const cols = [
      { name: "ID", dbType: "int", nullable: false },
      { name: "name", dbType: "text", nullable: true },
    ];
    expect(keyColumnsOf("mysql", cols, ["id"])).toEqual([0]);
    expect(keyColumnsOf("sqlite", cols, ["Id"])).toEqual([0]);
    expect(keyColumnsOf("postgres", cols, ["id"])).toBeUndefined();
    expect(
      keyColumnsOf("mysql", [...cols, { name: "id", dbType: "int", nullable: false }], ["id"]),
    ).toBeUndefined();
    // ASCII 만 무시한다.
    expect(sameColumnName("mysql", "İ", "i")).toBe(false);
  });

  it("turns typed text into the value the server reads, never a decimal", () => {
    expect(valueFromInput("5", undefined, { int: 1 })).toEqual({ int: 5 });
    expect(valueFromInput("12345678901234567", undefined, { int: 1 })).toEqual({
      text: "12345678901234567",
    });
    expect(valueFromInput("1.5", undefined, { float: 1 })).toEqual({ float: 1.5 });
    expect(valueFromInput("1.5", undefined, { decimal: "1.0" })).toEqual({ text: "1.5" });
    expect(valueFromInput("x", undefined, { int: 1 })).toEqual({ text: "x" });
    expect(
      valueFromInput("0xAB01", { name: "b", dbType: "bytea", nullable: true }, "null"),
    ).toEqual({
      bytes: { hex: "0xab01", size: 2, truncated: false },
    });
    expect(valueFromInput("0xA", { name: "b", dbType: "bytea", nullable: true }, "null")).toEqual({
      text: "0xA",
    });
    expect(valueFromInput("", undefined, undefined)).toEqual({ text: "" });
  });

  it("does not edit a value that was cut short", () => {
    expect(editableValue({ truncated: { prefix: "a", size: 9 } } as DbValue)).toBe(false);
    expect(editableValue({ bytes: { hex: "0x00", size: 9, truncated: true } })).toBe(false);
    expect(editableValue({ text: "a" })).toBe(true);
    expect(editableValue(DEFAULT_CELL)).toBe(true);
  });

  it("builds updates by the read key, inserts without default cells, deletes by key — deletes win over edits", () => {
    const b = loaded([
      [{ int: 1 }, { text: "one" }, "null"],
      [{ int: 2 }, { text: "two" }, "null"],
      [{ int: 3 }, { text: "three" }, "null"],
    ]);
    b.setCell(0, 1, { text: "uno" }, sameValue);
    b.setCell(2, 1, { text: "tres" }, sameValue);
    b.toggleRemove([2]);
    const added = b.addRow();
    expect(added).toBe(3);
    b.setCell(added, 1, { text: "four" }, sameValue);
    b.toggleRemove([1]);
    expect(b.pendingCount).toBe(4);
    expect(buildChanges(b, [0])).toEqual({
      updates: [
        {
          key: [{ column: "id", value: { int: 1 } }],
          values: [{ column: "name", value: { text: "uno" } }],
        },
      ],
      inserts: [{ values: [{ column: "name", value: { text: "four" } }] }],
      deletes: [
        { key: [{ column: "id", value: { int: 2 } }] },
        { key: [{ column: "id", value: { int: 3 } }] },
      ],
    });
    expect(buildChanges(loaded([]), [0])).toEqual({});
  });
});

describe("EditBuffer", () => {
  it("overlays edits and added rows on the read rows and marks them", () => {
    const b = loaded([[{ int: 1 }, { text: "one" }, "null"]]);
    let heard = 0;
    b.subscribe(() => heard++);
    b.setCell(0, 1, { text: "uno" }, sameValue);
    expect(b.row(0)).toEqual([{ int: 1 }, { text: "uno" }, "null"]);
    expect(b.original(0)).toEqual([{ int: 1 }, { text: "one" }, "null"]);
    expect(b.mark(0, 1)).toBe("changed");
    expect(b.mark(0, 0)).toBeUndefined();
    b.addRow();
    expect(b.rowCount).toBe(2);
    expect(b.row(1)).toEqual([DEFAULT_CELL, DEFAULT_CELL, DEFAULT_CELL]);
    expect(b.mark(1, 2)).toBe("inserted");
    b.toggleRemove([0]);
    expect(b.mark(0, 1)).toBe("deleted");
    // 지울 행은 고치지 않는다.
    b.setCell(0, 1, { text: "x" }, sameValue);
    expect(b.row(0)![1]).toEqual({ text: "uno" });
    // 다시 누르면 되살린다(지울 행만 골랐으면), 추가 행은 없앤다.
    b.toggleRemove([0, 1]);
    expect(b.rowCount).toBe(1);
    expect(b.isRemoved(0)).toBe(false);
    // 원래 값으로 돌아가면 고침이 없어진다.
    b.setCell(0, 1, { text: "one" }, sameValue);
    expect(b.pendingCount).toBe(0);
    expect(heard).toBeGreaterThan(0);
    b.addRow();
    b.revert();
    expect(b.rowCount).toBe(1);
    expect(b.pendingCount).toBe(0);
  });

  it("hands header sorting to the screen instead of sorting the held rows", () => {
    const b = loaded([
      [{ int: 2 }, { text: "b" }, "null"],
      [{ int: 1 }, { text: "a" }, "null"],
    ]);
    const asked: unknown[] = [];
    b.onSort = (s) => asked.push(s);
    b.setSort({ column: 0, direction: "asc" });
    expect(asked).toEqual([{ column: 0, direction: "asc" }]);
    expect(b.row(0)![0]).toEqual({ int: 2 });
    expect(b.sort).toBeUndefined();
    b.showSort({ column: 1, direction: "desc" });
    expect(b.sort).toEqual({ column: 1, direction: "desc" });
  });
});

describe("editFailureText", () => {
  const err = (code: number, data: object) =>
    new RpcCallError({ code, message: "m", data: data as never });

  it("names the change and what was undone", () => {
    expect(
      editFailureText(
        err(ERROR_CODE.CONFLICT, {
          reason: "rowCount",
          failure: {
            statement: 1,
            change: { kind: "delete", index: 1 },
            rowsAffected: 0,
            undone: "savepoint",
          },
        }),
        T.fail,
        S.refusal,
      ),
    ).toBe(`${T.fail.rowCount("삭제 2", 0)} ${T.fail.undone.savepoint}`);
    expect(
      editFailureText(err(ERROR_CODE.CONFLICT, { reason: "previewChanged" }), T.fail, S.refusal),
    ).toBe(T.fail.changed);
    expect(
      editFailureText(
        err(3003, {
          error: { kind: "query", message: "duplicate key" },
          failure: {
            statement: 0,
            change: { kind: "insert", index: 0 },
            rowsAffected: null,
            undone: "edit",
          },
        }),
        T.fail,
        S.refusal,
      ),
    ).toBe(`추가 1 — ${T.fail.db("duplicate key")} ${T.fail.undone.edit}`);
    expect(
      editFailureText(
        err(ERROR_CODE.INVALID_PARAMS, {
          reason: "keyColumnChanged",
          column: "id",
          change: { kind: "update", index: 0 },
        }),
        T.fail,
        S.refusal,
      ),
    ).toBe(`수정 1 — ${T.fail.invalid("keyColumnChanged", " · id")}`);
    expect(
      editFailureText(
        err(ERROR_CODE.PERMISSION_DENIED, { reason: "readOnlyConnection" }),
        T.fail,
        S.refusal,
      ),
    ).toBe(S.refusal.readOnlyConnection());
    expect(editFailureText(new Error("boom"), T.fail, S.refusal)).toBe("boom");
  });
});

describe("tab and action shapes", () => {
  it("accepts a table data tab only with a connection, an object and string fragments", () => {
    const spec = {
      kind: "tableData",
      tabId: "t1",
      connectionId: "dev",
      object: T_,
      where: "",
      orderBy: "a",
    };
    expect(isDbDocumentSpec(spec)).toBe(true);
    expect(isDbDocumentSpec({ ...spec, where: undefined, orderBy: undefined })).toBe(true);
    expect(isDbDocumentSpec({ ...spec, where: 1 })).toBe(false);
    expect(isDbDocumentSpec({ ...spec, connectionId: "" })).toBe(false);
    expect(isDbDocumentSpec({ ...spec, object: { ...T_, name: "" } })).toBe(false);
  });

  it("reads db.tableData.open args as { connectionId, object, where? } and nothing else", () => {
    expect(openArgs({ connectionId: "dev", object: T_ })).toEqual({
      connectionId: "dev",
      object: T_,
    });
    expect(openArgs({ connectionId: "dev", object: { ...T_, extra: 1 }, where: "id = 1" })).toEqual(
      {
        connectionId: "dev",
        object: T_,
        where: "id = 1",
      },
    );
    expect(openArgs({ connectionId: "dev" })).toBeUndefined();
    expect(openArgs({ connectionId: "", object: T_ })).toBeUndefined();
    expect(openArgs(undefined)).toBeUndefined();
  });
});
