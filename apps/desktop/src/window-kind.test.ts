import { describe, expect, it } from "vitest";

import { parseWindowKind } from "./window-kind";

describe("parseWindowKind", () => {
  it("reads the kind Rust puts in the query", () => {
    expect(parseWindowKind("?window=welcome")).toBe("welcome");
    expect(parseWindowKind("?window=project")).toBe("project");
    expect(parseWindowKind("?window=settings")).toBe("settings");
  });

  it("falls back to welcome for a missing or unknown kind", () => {
    expect(parseWindowKind("")).toBe("welcome");
    expect(parseWindowKind("?window=nope")).toBe("welcome");
  });
});
