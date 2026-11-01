// SSH 터널 시험(live_ssh_tunnel_*)의 sshd 를 띄운다(14-m3-plan.md M3-11). 클라이언트 키 · 호스트 키를 그 자리에서 만들고, 그 키를
// 단 sshd 컨테이너(안에 에코 서버 — 7000 번, 「터널 너머 가짜 TCP 서버」)를 띄운 뒤 시험이 읽을 환경 변수를 알린다.
//
//   node scripts/db-ssh.mjs [--out <폴더>] [--name <이름>] [--port <포트>]
//
// CI 의 `db-live` 잡은 checkout 뒤 이것을 부른다(저장소의 파일을 마운트하므로 서비스 컨테이너로 둘 수 없다). 환경 변수는
// `GITHUB_ENV` 가 있으면 거기에 더하고 늘 표준 출력에 `이름=값` 줄로 낸다. `mise run test-db` 는 도커가 있고 `PW_TEST_SSH_URL` 이
// 없으면 이 파일의 `startSsh` 로 워크트리별 이름 · 빈 포트에 띄우고 끝나면 끈다(`scripts/db-live.mjs`).
//
// - 이미지: alpine 을 판 · digest 로 고정하고 컨테이너가 시작할 때 `openssh-server` 를 설치한다(네트워크 필요 — 설치가 끝나 sshd
//   가 인사할 때까지는 시험의 준비 확인 `live_ssh_tunnel_ready` 가 120 s 까지 기다린다). 배포물이 아니라 라이선스는 따지지 않는다.
// - 키는 실행마다 새로 만든다 — 저장소에 키를 두지 않는다. known_hosts 는 `[127.0.0.1]:<포트>` 에 호스트 키 한 줄이다.
// - `--out`(기본 `target/db-ssh/`)은 도커가 마운트할 수 있는 폴더여야 한다(macOS Docker Desktop 은 `/private/tmp` 아래를 비워
//   보인다 — scripts/db-tls.mjs 와 같다).
// - 컨테이너는 `--rm` 이다 — 끝나면 `docker stop <이름>`. 같은 이름이 떠 있으면 실패한다(다른 워크트리의 서버를 끄지 않는다).

import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

// 판을 올리면 digest 도 함께.
const IMAGE = "alpine:3.22@sha256:2e0b81d60597f449baa56dc25d355367869d2dca4cd57e866f1e2b52af3eb55f";

export const URL_ENV = "PW_TEST_SSH_URL";
export const DIR_ENV = "PW_TEST_SSH_DIR";
const ECHO_PORT = 7000;

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout;
}

/** 클라이언트 키(`client`) · 호스트 키(`host_ed25519_key`) · authorized_keys 를 `out` 에 새로 만든다. */
export function makeKeys(out) {
  mkdirSync(out, { recursive: true });
  for (const f of ["client", "client.pub", "host_ed25519_key", "host_ed25519_key.pub"])
    rmSync(join(out, f), { force: true });
  run("ssh-keygen", [
    "-q",
    "-t",
    "ed25519",
    "-N",
    "",
    "-C",
    "pitwall-test",
    "-f",
    join(out, "client"),
  ]);
  run("ssh-keygen", [
    "-q",
    "-t",
    "ed25519",
    "-N",
    "",
    "-C",
    "pitwall-test-host",
    "-f",
    join(out, "host_ed25519_key"),
  ]);
  writeFileSync(join(out, "authorized_keys"), readFileSync(join(out, "client.pub")));
}

/** 컨테이너 안에서 도는 명령: sshd 설치 · 사용자 · 키 · 에코 서버 · sshd. */
function entry() {
  return [
    "apk add --no-cache openssh-server >/dev/null",
    "adduser -D -s /bin/sh pitwall",
    // 잠긴 계정은 공개 키로도 들어가지 못한다 — 암호 없이 연다.
    "passwd -u pitwall >/dev/null 2>&1 || true",
    "install -d -o pitwall -m 700 /home/pitwall/.ssh",
    "install -o pitwall -m 600 /pw-ssh/authorized_keys /home/pitwall/.ssh/authorized_keys",
    "install -m 600 /pw-ssh/host_ed25519_key /etc/ssh/ssh_host_ed25519_key",
    `(nc -lk -p ${ECHO_PORT} -e cat &)`,
    // 출처별 벌점(OpenSSH 9.8+ 기본값)을 끈다. 시험은 인증 없이 끊는 접속(준비 확인 · 모르는 호스트 키에서 멈추는 ssh)을 실행마다
    // 열 개 넘게 하고 그것이 한 출처에 쌓여 15초를 넘으면 sshd 가 그 출처의 접속을 곧바로 닫는다 — 같은 sshd 로 다시 돌리면 거의 다
    // 「Connection closed」 로 실패한다.
    "exec /usr/sbin/sshd -D -e -h /etc/ssh/ssh_host_ed25519_key -o AllowTcpForwarding=yes -o PasswordAuthentication=no -o PerSourcePenalties=no",
  ].join(" && ");
}

/** 키가 `out` 에 있을 때 sshd 를 띄운다. 포트 0 은 빈 포트. 띄운 이름 · 포트와 시험에 넘길 변수를 돌려준다. */
export function startSsh({ name, port, out }) {
  run("docker", [
    "run",
    "-d",
    "--rm",
    "--name",
    name,
    "-p",
    port === 0 ? "127.0.0.1::22" : `127.0.0.1:${port}:22`,
    "-v",
    `${out}:/pw-ssh:ro`,
    IMAGE,
    "sh",
    "-c",
    entry(),
  ]);
  let bound = port;
  if (port === 0) {
    const line = run("docker", ["port", name, "22/tcp"]).trim().split("\n")[0];
    bound = Number(line.split(":").at(-1));
  }
  const hostKey = readFileSync(join(out, "host_ed25519_key.pub"), "utf8").split(" ").slice(0, 2);
  writeFileSync(join(out, "known_hosts"), `[127.0.0.1]:${bound} ${hostKey.join(" ")}\n`);
  return {
    name,
    port: bound,
    env: { [URL_ENV]: `ssh://pitwall@127.0.0.1:${bound}`, [DIR_ENV]: out },
  };
}

/** 그 이름의 컨테이너를 끈다(`--rm` 이라 지워진다). 없으면 아무것도 하지 않는다. */
export function stopSsh(name) {
  spawnSync("docker", ["stop", name], { encoding: "utf8" });
}

export function parseArgs(argv) {
  const options = { out: join(root, "target", "db-ssh"), name: "pitwall-ssh", port: 0 };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--out") options.out = resolve(argv[++i]);
    else if (arg === "--name") options.name = argv[++i];
    else if (arg === "--port") {
      const port = argv[++i] ?? "";
      if (!/^\d+$/.test(port)) throw new Error(`--port expects a number, got ${port}`);
      options.port = Number(port);
    } else throw new Error(`unknown argument ${arg}`);
  }
  return options;
}

function main(argv) {
  const options = parseArgs(argv);
  makeKeys(options.out);
  const started = startSsh(options);
  console.error(`ssh: started ${started.name} on 127.0.0.1:${started.port}`);
  const lines = Object.entries(started.env).map(([key, value]) => `${key}=${value}`);
  if (process.env.GITHUB_ENV) appendFileSync(process.env.GITHUB_ENV, `${lines.join("\n")}\n`);
  console.log(lines.join("\n"));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`db-ssh: ${e.message}`);
    process.exitCode = 2;
  }
}
