// scripts/db-tls.mjs 의 인자 읽기를 도커 · openssl 없이 본다.
// 실행: node --test scripts/db-tls.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

import { CA_ENV, TARGETS, makeCertificates, parseArgs, publish } from "./db-tls.mjs";

test("no db means every target, on its default port and name", () => {
  const options = parseArgs([]);
  assert.deepEqual(options.dbs, Object.keys(TARGETS));
  assert.deepEqual(options.ports, {});
  assert.equal(options.name, "pitwall-tls");
  assert.match(options.out, /target[\\/]db-tls$/);
});

test("a worktree picks its own name, port and folder", () => {
  const options = parseArgs([
    "--name",
    "pw-m306-tls",
    "--port",
    "pg=55437",
    "--out",
    "x/certs",
    "pg",
  ]);
  assert.deepEqual(options.dbs, ["pg"]);
  assert.deepEqual(options.ports, { pg: 55437 });
  assert.equal(options.name, "pw-m306-tls");
  assert.equal(options.out, resolve("x/certs"));
});

test("unknown dbs and bad ports are refused", () => {
  assert.throws(() => parseArgs(["oracle"]), /unknown argument/);
  assert.throws(() => parseArgs(["--port", "pg=abc"]), /--port expects/);
  assert.throws(() => parseArgs(["--port", "mssql=1433"]), /--port expects/);
});

test("the pg target hands the tests a localhost URL, and the CA is shared", () => {
  assert.equal(CA_ENV, "PW_TEST_TLS_CA");
  assert.equal(TARGETS.pg.urlEnv, "PW_TEST_PG_TLS_URL");
  assert.equal(TARGETS.pg.url(5443), "postgres://pitwall:pitwall@localhost:5443/pitwall");
  const args = TARGETS.pg.args({ out: "/certs" });
  assert.ok(args.includes("/certs:/pw-tls:ro"), args.join(" "));
  assert.ok(args.at(-1).includes("ssl=on"));
});

test("the redis target is tls-only on a pinned image and shares the CA", () => {
  assert.equal(TARGETS.redis.urlEnv, "PW_TEST_REDIS_TLS_URL");
  assert.equal(TARGETS.redis.url(6380), "rediss://localhost:6380/0");
  const args = TARGETS.redis.args({ out: "/certs" });
  assert.ok(args.includes("/certs:/pw-tls:ro"), args.join(" "));
  assert.ok(
    args.some((a) => /^redis:[\d.]+@sha256:[0-9a-f]{64}$/.test(a)),
    args.join(" "),
  );
  const command = args.at(-1);
  assert.match(command, /--port 0 --tls-port 6380 /);
  assert.match(command, /--tls-ca-cert-file \/tls\/ca\.pem/);
  assert.equal(TARGETS.redis.innerPort, 6380);
});

test("port 0 lets docker pick a free port", () => {
  assert.equal(publish(0, 5432), "127.0.0.1::5432");
  assert.equal(publish(5443, 5432), "127.0.0.1:5443:5432");
});

// openssl 이 없는 환경(예: Windows 러너)에서는 건너뛰고 그렇게 알린다.
const hasOpenssl = spawnSync("openssl", ["version"]).status === 0;
test(
  "makeCertificates writes the serial file into the certificate folder, not the cwd",
  { skip: hasOpenssl ? false : "openssl not found" },
  () => {
    // LibreSSL 은 CA 경로의 첫 `.` 부터를 `.srl` 로 바꿔 일련번호 파일 이름을 만든다. 경로에 점이 든 폴더(`.worktrees/…` 등)가 있으면 이름이 비어
    // `.srl` 이 cwd 에 생긴다 — 그 조건을 만든다.
    const cwd = mkdtempSync(join(tmpdir(), "db-tls-cwd-"));
    const before = process.cwd();
    process.chdir(cwd);
    try {
      makeCertificates(".certs");
    } finally {
      process.chdir(before);
    }
    assert.deepEqual(readdirSync(cwd), [".certs"], "nothing was left in the cwd");
    assert.ok(existsSync(join(cwd, ".certs", "ca.srl")));
    assert.ok(existsSync(join(cwd, ".certs", "server.pem")));
  },
);
