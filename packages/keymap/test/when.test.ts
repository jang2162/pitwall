import { describe, expect, it } from "vitest";

import { evaluateWhen, parseWhen, WhenParseError, whenDepth } from "../src/when";

const ctx = (...layers: Record<string, string | number | boolean>[]) => layers;

describe("when", () => {
  it("evaluates keys, negation, and/or and parentheses", () => {
    const layers = ctx({ editorFocus: true }, { toolWindow: "project" }, { isMac: true });
    expect(evaluateWhen(parseWhen("editorFocus"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("!editorFocus"), layers)).toBe(false);
    expect(evaluateWhen(parseWhen("missing"), layers)).toBe(false);
    expect(evaluateWhen(parseWhen("editorFocus && !terminalFocus"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("terminalFocus || isMac"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("!(editorFocus && isMac)"), layers)).toBe(false);
    // && 가 || 보다 먼저 묶인다.
    expect(evaluateWhen(parseWhen("terminalFocus && isMac || editorFocus"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("terminalFocus && (isMac || editorFocus)"), layers)).toBe(false);
  });

  it("compares values by type", () => {
    const layers = ctx({ toolWindow: "project", count: 2, flag: true });
    expect(evaluateWhen(parseWhen("toolWindow == 'project'"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("toolWindow == project"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen('toolWindow != "git"'), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("count == 2"), layers)).toBe(true);
    expect(evaluateWhen(parseWhen("count == '2'"), layers)).toBe(false);
    expect(evaluateWhen(parseWhen("flag == true"), layers)).toBe(true);
  });

  it("lets an inner layer shadow an outer one", () => {
    expect(evaluateWhen(parseWhen("readonly"), ctx({ readonly: false }, { readonly: true }))).toBe(
      false,
    );
  });

  it.each(["", "a &&", "(a", "a == ", "a =~ /x/", "a < 3", "'x'", "a b"])("rejects %j", (text) => {
    expect(() => parseWhen(text)).toThrow(WhenParseError);
  });

  it("measures how inner the keys a condition reads are", () => {
    const layers = ctx({ terminalFocus: true }, { toolWindowFocus: true }, { isMac: true });
    expect(whenDepth(undefined, layers)).toBe(0);
    expect(whenDepth(parseWhen("isMac"), layers)).toBe(1);
    expect(whenDepth(parseWhen("toolWindowFocus"), layers)).toBe(2);
    expect(whenDepth(parseWhen("terminalFocus && isMac"), layers)).toBe(3);
    expect(whenDepth(parseWhen("!unknownKey"), layers)).toBe(0);
  });
});
