// java-debug-patch.mjs 의 판정(결정 D71): 패치는 바인드 주소 한 줄만, 커밋된 소스는 원본 + 패치, registry.toml 의 [[patched]] 행.
// JDK · 네트워크 없이 돈다.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  applyPatch,
  checkSource,
  parsePatch,
  parseUpstream,
  patchedRows,
  withPatchedRow,
} from "./java-debug-patch.mjs";
import { patchedBlocks } from "./tools-registry-bump.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const DIR = join(root, "crates", "pw-tools", "patches", "java-debug");
const read = (name) => readFileSync(join(DIR, name), "utf8");
const real = () => ({
  upstream: parseUpstream(read("upstream.toml")),
  patchText: read("loopback.patch"),
  patched: read("JavaDebugServer.java"),
});

test("the committed source is the pinned upstream file plus the one-line patch", () => {
  assert.deepEqual(checkSource(real()), []);
  const patch = parsePatch(real().patchText);
  assert.match(patch.before, /new ServerSocket\(0, 1\);$/);
  assert.match(
    patch.after,
    /new ServerSocket\(0, 1, java\.net\.InetAddress\.getLoopbackAddress\(\)\);$/,
  );
});

test("any other change to the source is caught", () => {
  const r = real();
  const touched = r.patched.replace("ThreadPoolExecutor(0, 100,", "ThreadPoolExecutor(0, 5,");
  assert.notEqual(touched, r.patched);
  const problems = checkSource({ ...r, patched: touched });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /differs from upstream beyond the patch/);

  // 패치 줄을 되돌린(상류 그대로) 소스는 문맥이 맞지 않는다.
  const unpatched = applyPatch(r.patched, parsePatch(r.patchText), true);
  assert.match(checkSource({ ...r, patched: unpatched })[0], /matches 0 places/);
});

test("a patch that changes more than the bind line is refused", () => {
  const { patchText } = real();
  // 두 줄을 바꾼다.
  const two = patchText.replace(
    "         } catch (IOException e) {",
    "-        } catch (IOException e) {\n+        } catch (Exception e) {",
  );
  assert.throws(() => parsePatch(two), /not one line/);
  // 한 줄이지만 다른 것으로 바꾼다(모든 주소에 그대로 묶는 다른 인자).
  const other = patchText.replace(
    "java.net.InetAddress.getLoopbackAddress()",
    'java.net.InetAddress.getByName("0.0.0.0")',
  );
  assert.throws(() => parsePatch(other), /not the loopback bind/);
  // 덩어리가 둘이다.
  assert.throws(() => parsePatch(`${patchText}@@ -1,1 +1,1 @@\n x\n`), /2 hunks/);
});

test("the [[patched]] row is written once and survives a registry bump", () => {
  const base = `bundled = []\n\n[[artifact]]\npackage = "x"\n`;
  const row = {
    package: "java-debug-adapter",
    version: "0.59.0",
    file: "extension/server/a.jar",
    patch: "java-debug-loopback",
    sha256: "a".repeat(64),
    patched_sha256: "b".repeat(64),
  };
  const once = withPatchedRow(base, row);
  const twice = withPatchedRow(once, { ...row, patched_sha256: "c".repeat(64) });
  assert.deepEqual(patchedRows(once), [row]);
  assert.deepEqual(patchedRows(twice), [{ ...row, patched_sha256: "c".repeat(64) }]);
  assert.ok(twice.startsWith(base.trimEnd()));
  // bump 은 [[patched]] 를 만들지 않고 글 그대로 옮긴다.
  const blocks = patchedBlocks(twice);
  assert.equal(blocks.length, 1);
  assert.deepEqual(patchedRows(`${base}\n${blocks[0]}\n`), patchedRows(twice));
});

test("the real registry.toml pins the patch for the bundled java-debug", () => {
  const rows = patchedRows(readFileSync(join(root, "crates", "pw-tools", "registry.toml"), "utf8"));
  const up = parseUpstream(read("upstream.toml"));
  assert.equal(rows.length, 1);
  assert.equal(rows[0].package, up.package);
  assert.equal(rows[0].version, up.version);
  assert.equal(rows[0].file, up.vsix_entry);
});
