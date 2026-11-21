// relay-live(18-m7-plan M7-05 ③): Linux sshd 컨테이너를 띄우고 실제 `ssh` 로 Relay 를 올려 붙는 시험(apps/relay/tests/live.rs)을
// 돌린 뒤 실행 개수를 하한(scripts/relay-live.toml)과 견준다. CI 의 `relay-live` 잡과 `mise run test-relay` 가 부른다.
//
//   node scripts/relay-live.mjs [--ci] --bin <원격에 올릴 Relay 바이너리>
//
// - 컨테이너: alpine(musl — 정적 Relay 가 그대로 돈다)에 openssh-server · git 을 시작할 때 설치한다(네트워크 필요 — 시험의 준비
//   확인 `live_relay_ready` 가 180 s 까지 기다린다). 키는 실행마다 새로 만든다(scripts/db-ssh.mjs 의 makeKeys) — 저장소에 키를
//   두지 않는다. known_hosts 는 그 호스트 키 한 줄이고 시험은 호스트 키 확인을 끄지 않는다. 배포물이 아니라 라이선스는 따지지 않는다.
// - **이 기기의 sshd 는 쓰지 않는다** — 도커가 없으면 로컬은 「건너뜀」(종료 코드 3, 통과로 세지 않는다), `--ci` 는 실패다.
// - 시험은 `-j 1` 로 돈다 — 끊김 시나리오가 컨테이너를 얼리면(`docker pause`) 같은 sshd 를 쓰는 다른 시험이 함께 멈춘다.
// - 원격 컨테이너의 아키텍처는 이 기기(도커 호스트)의 것이다. `--bin` 은 그 아키텍처의 musl 바이너리여야 한다(CI: x86_64 —
//   `node scripts/relay-build.mjs --target x86_64-unknown-linux-musl`).
// - 컨테이너는 `--rm` 이고 끝나면 — 실패 · Ctrl-C 포함 — 이름으로 끈다(이 워크트리 · 이 실행만의 이름).

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { filterExpr, judge, parseFloors, runCount, selectedTests } from "./db-live.mjs";
import { makeKeys } from "./db-ssh.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

// scripts/db-ssh.mjs 와 같은 이미지(판 · digest 고정). 판을 올리면 둘을 함께.
const IMAGE = "alpine:3.22@sha256:2e0b81d60597f449baa56dc25d355367869d2dca4cd57e866f1e2b52af3eb55f";

export const FILTER = "live_relay";
export const URL_ENV = "PW_TEST_RELAY_SSH_URL";

export function parseArgs(argv) {
  const options = { ci: false, bin: null };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--ci") options.ci = true;
    else if (arg === "--bin") options.bin = resolve(argv[++i] ?? "");
    else throw new Error(`unknown argument ${arg}`);
  }
  if (!options.bin) throw new Error("--bin <relay binary for the container> is required");
  return options;
}

/** 이 워크트리 · 이 실행만의 컨테이너 이름. */
export function containerName(rootPath, pid) {
  const worktree = createHash("sha256").update(rootPath).digest("hex").slice(0, 8);
  return `pitwall-relay-live-${worktree}-${pid}`;
}

/** 컨테이너 안에서 도는 명령: sshd · git 설치, 사용자 · 키, sshd. */
export function entry() {
  return [
    "apk add --no-cache openssh-server git coreutils >/dev/null",
    "adduser -D -s /bin/sh pitwall",
    "passwd -u pitwall >/dev/null 2>&1 || true",
    "install -d -o pitwall -m 700 /home/pitwall/.ssh",
    "install -o pitwall -m 600 /pw-ssh/authorized_keys /home/pitwall/.ssh/authorized_keys",
    "install -m 600 /pw-ssh/host_ed25519_key /etc/ssh/ssh_host_ed25519_key",
    // 준비 확인 · 끊김 시나리오의 접속이 쌓여 sshd 의 출처별 벌점에 걸리지 않게 끈다(scripts/db-ssh.mjs 와 같은 까닭).
    "exec /usr/sbin/sshd -D -e -h /etc/ssh/ssh_host_ed25519_key -o PasswordAuthentication=no -o PerSourcePenalties=no",
  ].join(" && ");
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout;
}

function dockerAvailable() {
  const result = spawnSync("docker", ["info", "--format", "{{.ServerVersion}}"], {
    encoding: "utf8",
  });
  return !result.error && result.status === 0;
}

function startContainer(name, out) {
  makeKeys(out);
  run("docker", [
    "run",
    "-d",
    "--rm",
    "--name",
    name,
    "-p",
    "127.0.0.1::22",
    "-v",
    `${out}:/pw-ssh:ro`,
    IMAGE,
    "sh",
    "-c",
    entry(),
  ]);
  const line = run("docker", ["port", name, "22/tcp"]).trim().split("\n")[0];
  const port = Number(line.split(":").at(-1));
  const hostKey = readFileSync(join(out, "host_ed25519_key.pub"), "utf8").split(" ").slice(0, 2);
  writeFileSync(join(out, "known_hosts"), `[127.0.0.1]:${port} ${hostKey.join(" ")}\n`);
  return {
    [URL_ENV]: `ssh://pitwall@127.0.0.1:${port}`,
    PW_TEST_RELAY_SSH_DIR: out,
    PW_TEST_RELAY_CONTAINER: name,
  };
}

function cargo(args, env) {
  const result = spawnSync("cargo", args, {
    cwd: root,
    env,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) throw result.error;
  return result;
}

function runTests(env, floor) {
  const expr = filterExpr(FILTER);
  const list = cargo(
    ["nextest", "list", "-p", "pitwall-relay", "-E", expr, "--message-format", "json"],
    env,
  );
  if (list.status !== 0) {
    process.stderr.write(list.stderr);
    return { ok: false, message: `relay: cargo nextest list failed (exit ${list.status})` };
  }
  const selected = selectedTests(JSON.parse(list.stdout));
  for (const name of selected) console.log(`   selected: ${name}`);
  const result = cargo(
    [
      "nextest",
      "run",
      "-p",
      "pitwall-relay",
      "-E",
      expr,
      "--no-tests=fail",
      "-j",
      "1",
      "--no-fail-fast",
      "--color",
      "never",
    ],
    env,
  );
  process.stdout.write(result.stdout);
  process.stderr.write(result.stderr);
  return judge({
    db: "relay",
    filter: FILTER,
    selected,
    run: runCount(result.stderr + result.stdout),
    floor,
    exitCode: result.status,
  });
}

/** 하한 파일의 relay 줄. */
export function relayFloor(text) {
  const floors = parseFloors(text);
  if (!floors.has("relay"))
    throw new Error("scripts/relay-live.toml has no `relay = <count>` line");
  if (floors.size !== 1) throw new Error("scripts/relay-live.toml has lines other than `relay`");
  return floors.get("relay");
}

function main(argv) {
  const options = parseArgs(argv);
  const floor = relayFloor(readFileSync(join(root, "scripts", "relay-live.toml"), "utf8"));
  if (!existsSync(options.bin)) throw new Error(`no relay binary at ${options.bin}`);
  if (!dockerAvailable()) {
    const message = "relay-live: docker is not available — not run";
    if (options.ci) {
      console.error(`${message} (the CI job needs it)`);
      return 1;
    }
    console.log(`${message}; skipped, not counted as passed`);
    return 3;
  }
  const name = containerName(root, process.pid);
  const out = join(root, "target", "relay-live-ssh");
  let stopped = false;
  const stop = () => {
    if (stopped) return;
    stopped = true;
    spawnSync("docker", ["stop", name], { encoding: "utf8" });
    console.log(`== stopped ${name}`);
  };
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.once(signal, () => {
      stop();
      process.exit(130);
    });
  }
  try {
    const server = startContainer(name, out);
    console.log(`== relay-live: started ${name} (${server[URL_ENV]})`);
    const env = { ...process.env, ...server, PW_TEST_RELAY_BIN: options.bin };
    const result = runTests(env, floor);
    console.log(result.message);
    return result.ok ? 0 : 1;
  } finally {
    stop();
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (e) {
    console.error(`relay-live: ${e.message}`);
    process.exitCode = 2;
  }
}
