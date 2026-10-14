import { describe, expect, it } from "vitest";

import { packageName } from "./index";

describe("packageName", () => {
  it("returns the package name", () => {
    expect(packageName()).toBe("@pitwall/domain");
  });
});
