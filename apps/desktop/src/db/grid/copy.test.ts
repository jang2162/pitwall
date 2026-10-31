// 복사 형식(M3-16): CSV 는 RFC 4180 의 따옴표 규칙대로, TSV 는 스프레드시트 붙여넣기 꼴, JSON 은 정밀도를 지키고, INSERT 는
// Host 의 `db.sql.insertText`(리터럴 규칙의 정본) — 잘린 셀이 있으면 Host 에 보내지 않고 거절한다.

import { RpcCallError, type RpcClient } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { type CellBlock, CopyRefused, toCsv, toInsert, toJson, toTsv } from "./copy";

const block: CellBlock = {
  columns: [
    { name: "id", dbType: "int8", nullable: false },
    { name: 'na,"me', dbType: "text", nullable: true },
  ],
  rows: [
    [{ int: 1 }, { text: 'say "hi", ok' }],
    [{ int: "9007199254740993" }, "null"],
    [{ int: 3 }, { text: "two\r\nlines" }],
    [{ int: 4 }, { text: "" }],
  ],
};

describe("csv", () => {
  it('follows RFC 4180: header row, CRLF, quotes around , " CR LF and doubled quotes', () => {
    expect(toCsv(block)).toBe(
      'id,"na,""me"\r\n' +
        '1,"say ""hi"", ok"\r\n' +
        "9007199254740993,\r\n" +
        '3,"two\r\nlines"\r\n' +
        "4,\r\n",
    );
  });

  it("leaves plain fields unquoted", () => {
    expect(toCsv({ columns: [block.columns[0]!], rows: [[{ text: "a b" }]] })).toBe(
      "id\r\na b\r\n",
    );
  });
});

describe("tsv", () => {
  it("joins cells with tabs and quotes cells holding tabs, newlines or quotes", () => {
    expect(toTsv(block)).toBe('1\t"say ""hi"", ok"\n9007199254740993\t\n3\t"two\r\nlines"\n4\t');
  });
});

describe("json", () => {
  it("keeps big integers as strings, parses json values and renames duplicate columns", () => {
    const out = JSON.parse(
      toJson({
        columns: [
          { name: "a", dbType: "int8", nullable: null },
          { name: "a", dbType: "jsonb", nullable: null },
          { name: "b", dbType: "bytea", nullable: null },
        ],
        rows: [
          [
            { int: "9007199254740993" },
            { json: '{"k":[1,2]}' },
            { bytes: { hex: "0x00ff", size: 2, truncated: false } },
          ],
          [{ int: 2 }, { json: "not json" }, "null"],
        ],
      }),
    ) as unknown;
    expect(out).toEqual([
      { a: "9007199254740993", a_2: { k: [1, 2] }, b: "0x00ff" },
      { a: 2, a_2: "not json", b: null },
    ]);
  });
});

describe("insert", () => {
  function client(call: (method: string, params: unknown) => Promise<unknown>) {
    return { call: vi.fn(call) } as unknown as RpcClient & { call: ReturnType<typeof vi.fn> };
  }

  it("asks the Host for the text with the dialect, table and rows", async () => {
    const c = client(async () => ({ text: "INSERT …;\n" }));
    const text = await toInsert(block, { client: c, dialect: "postgres", table: null });
    expect(text).toBe("INSERT …;\n");
    expect(c.call).toHaveBeenCalledWith("db.sql.insertText", {
      dialect: "postgres",
      table: null,
      columns: ["id", 'na,"me'],
      rows: block.rows,
    });
  });

  it("refuses a block with a truncated cell without calling the Host", async () => {
    const c = client(async () => ({ text: "" }));
    const truncated: CellBlock = {
      columns: block.columns,
      rows: [[{ int: 1 }, { truncated: { of: "text", prefix: "ab", size: 9e6, typeName: null } }]],
    };
    await expect(toInsert(truncated, { client: c, dialect: "mysql", table: null })).rejects.toEqual(
      new CopyRefused("truncated", { row: 0, column: 1 }),
    );
    expect(c.call).not.toHaveBeenCalled();
  });

  it("names the cell the Host refused", async () => {
    const c = client(async () => {
      throw new RpcCallError({
        code: -32602,
        message: "the dialect has no literal for this value",
        data: { row: 1, column: 0, reason: "unsupported" },
      });
    });
    const error = await toInsert(block, { client: c, dialect: "sqlite", table: null }).catch(
      (e) => e,
    );
    expect(error).toBeInstanceOf(CopyRefused);
    expect((error as CopyRefused).reason).toBe("unsupported");
    expect((error as CopyRefused).cell).toEqual({ row: 1, column: 0 });
  });
});
