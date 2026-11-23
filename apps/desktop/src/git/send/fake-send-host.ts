// 시험용 가짜 Host 의 git 쪽(커밋 보내기 · 가져오기 — 18-m7-plan M7-17 웹뷰 시험). **실제 범위 규칙대로 거절한다**(M7-11 리뷰 B1 —
// 범위를 흉내 내지 않는 가짜가 실앱의 거절을 가렸다):
//
// - 쓰기(`git.send.receive.*` · `git.send.apply` · `git.reset`)는 그 프로젝트 창의 연결만(`pw_core::registry::state::scope_may_write` —
//   `Project(id)` 이고 id 가 그 프로젝트). 다른 범위면 `-32001`.
// - 그 머신에 없는 프로젝트 id 면 `-32602`(레지스트리 `NotFound` 가 invalid params 로 나간다).
// - `receive.begin` 의 `sourceMachine` 은 엔진의 이름 규칙(`[A-Za-z0-9._-]` 1~128자, `.` 시작 · `.lock` 끝 없음)을 지켜야 한다.
// - 받기(`uploadId`)는 연 연결의 것이다(`UploadOwner`) — 다른 연결의 청크 · finish 는 not found.
// - 경로가 바뀌는 `apply` 는 그 프로젝트의 `git.prompts` 를 듣는 연결에 미저장 확인을 묻는다. 듣는 연결이 없으면 `2006`(NO_WINDOW),
//   `cancel` 이면 `-32002` 로 아무것도 하지 않는다(crates/pw-git/src/write/mod.rs).
// - `git.reset` 은 `expected`(HEAD 커밋) · `expectedRef`(HEAD 의 ref)를 주었고 지금과 다르면 `-32004` 로 아무것도 하지 않는다(선택 칸 —
//   주지 않으면 대조하지 않는다).
//
// 커밋은 「id → 부모」 표다. 묶음 바이트는 보낼 커밋들의 JSON(base64)이고, 여러 청크로 나뉜다.

/** 연결 하나의 범위. */
export type FakeScope = { kind: "host" } | { kind: "project"; projectId: string };

export interface FakeRun {
  worktree: string | null;
  state: "running" | "stopping" | "exited";
}

export interface FakeRepo {
  machine: string;
  projectId: string;
  workTree: string;
  /** id → 부모. */
  commits: Map<string, string | null>;
  branches: Map<string, string>;
  /** HEAD 의 브랜치. */
  head: string;
  incoming: Map<string, string>;
  /** 커밋 안 한 변경(작업 트리 기준 경로). */
  dirty: string[];
  /** 연결 워크트리 꼭대기 → 그 브랜치. */
  worktrees: Map<string, string>;
  runs: FakeRun[];
  terminals: FakeRun[];
  /** 받은 호출(메서드 · 파라미터). */
  calls: { method: string; params: Record<string, unknown> }[];
  /** 묶음 청크 크기(바이트). */
  chunk: number;
}

export function createRepo(machine: string, projectId: string, workTree: string): FakeRepo {
  return {
    machine,
    projectId,
    workTree,
    commits: new Map([["c0", null]]),
    branches: new Map([["main", "c0"]]),
    head: "main",
    incoming: new Map(),
    dirty: [],
    worktrees: new Map(),
    runs: [],
    terminals: [],
    calls: [],
    chunk: 24,
  };
}

/** 커밋 하나를 브랜치 끝에 더한다. */
export function commit(repo: FakeRepo, branch: string, id: string) {
  repo.commits.set(id, repo.branches.get(branch) ?? null);
  repo.branches.set(branch, id);
}

function ancestors(repo: FakeRepo, id: string): string[] {
  const out: string[] = [];
  let at: string | null | undefined = id;
  while (at) {
    out.push(at);
    at = repo.commits.get(at);
  }
  return out;
}

/** 연결 하나의 상태(받기 · 듣기). */
export interface FakeConnection {
  scope: FakeScope;
  send(message: object): void;
}

interface Upload {
  owner: FakeConnection;
  branch: string;
  commit: string;
  sourceMachine: string;
  size: number;
  data: string;
}

interface Listener {
  conn: FakeConnection;
  streamId: number;
}

/** 머신 하나의 가짜 git. */
export interface FakeGitHost {
  repo: FakeRepo;
  handle(
    conn: FakeConnection,
    id: number | undefined,
    method: string,
    params: Record<string, unknown>,
    nextStream: () => number,
  ): boolean;
  /** 연결이 닫혔다(그 연결의 받기 · 듣기를 버린다). */
  closed(conn: FakeConnection): void;
  /** 그 연결의 스트림 하나가 취소됐다(`stream.cancel`). */
  cancel(conn: FakeConnection, streamId: number): void;
  listeners(): number;
}

const error = (code: number, message: string, data: unknown = null) => ({ code, message, data });

export function createGitHost(repo: FakeRepo): FakeGitHost {
  const uploads = new Map<number, Upload>();
  let nextUpload = 1;
  let nextPrompt = 1;
  const listening = new Set<Listener>();
  const answers = new Map<number, (answer: { kind: string }) => void>();

  const mayWrite = (conn: FakeConnection, projectId: unknown) =>
    conn.scope.kind === "project" && conn.scope.projectId === projectId;

  function ask(paths: string[]): Promise<{ kind: string } | undefined> {
    const first = [...listening][0];
    if (!first) return Promise.resolve(undefined);
    const promptId = nextPrompt++;
    first.conn.send({
      method: "stream.chunk",
      params: {
        streamId: first.streamId,
        data: {
          kind: "open",
          prompt: { promptId, request: { kind: "unsaved", workTree: repo.workTree, paths } },
        },
      },
    });
    return new Promise((resolve) => answers.set(promptId, resolve));
  }

  const host: FakeGitHost = {
    repo,
    listeners: () => listening.size,
    cancel(conn, streamId) {
      for (const l of listening)
        if (l.conn === conn && l.streamId === streamId) listening.delete(l);
    },
    closed(conn) {
      for (const [id, u] of uploads) if (u.owner === conn) uploads.delete(id);
      for (const l of listening) if (l.conn === conn) listening.delete(l);
    },
    handle(conn, id, method, params, nextStream) {
      const reply = (result: unknown) => conn.send({ id, result });
      const fail = (e: ReturnType<typeof error>) => conn.send({ id, error: e });
      if (!method.startsWith("git.") && method !== "run.list" && method !== "terminal.list")
        return false;
      repo.calls.push({ method, params });
      const projectScoped =
        method !== "git.send.receive.chunk" && method !== "git.send.receive.finish";
      if (projectScoped && params.projectId !== repo.projectId) {
        fail(error(-32602, `project not found: ${String(params.projectId)}`));
        return true;
      }
      const writes = ["git.send.receive.begin", "git.send.apply", "git.reset", "git.prompt.answer"];
      if (writes.includes(method) && !mayWrite(conn, params.projectId)) {
        fail(error(-32001, "only the project's own window may write to its git repository"));
        return true;
      }
      const tipOfHead = () => repo.branches.get(repo.head) ?? null;
      switch (method) {
        case "git.prompts": {
          if (!mayWrite(conn, params.projectId)) {
            fail(error(-32001, "only the project's own window listens"));
            return true;
          }
          const streamId = nextStream();
          listening.add({ conn, streamId });
          reply({ streamId });
          return true;
        }
        case "git.prompt.answer": {
          const done = answers.get(params.promptId as number);
          answers.delete(params.promptId as number);
          if (!done) {
            fail(error(-32602, "no such prompt"));
            return true;
          }
          done(params.answer as { kind: string });
          reply(null);
          return true;
        }
        case "git.refs":
          reply({
            workTree: repo.workTree,
            head: { kind: "branch", name: repo.head, commit: tipOfHead() },
            branches: [...repo.branches].map(([name, commit]) => ({
              name,
              refName: `refs/heads/${name}`,
              commit,
              head: name === repo.head,
              upstream: null,
            })),
            remoteBranches: [],
            tags: [],
            unsupported: null,
            broken: [],
          });
          return true;
        case "git.status":
          reply({
            workTree: repo.workTree,
            entries: repo.dirty.map((path) => ({
              path,
              origPath: null,
              staged: null,
              unstaged: "modified",
              conflict: null,
            })),
            unsupported: null,
          });
          return true;
        case "git.worktrees":
          reply({
            workTree: repo.workTree,
            worktrees: [
              {
                path: repo.workTree,
                head: { kind: "branch", name: repo.head, commit: tipOfHead() },
                main: true,
                current: true,
                locked: false,
                missing: false,
                switchTo: { worktree: null },
              },
              ...[...repo.worktrees].map(([path, branch]) => ({
                path,
                head: { kind: "branch", name: branch, commit: repo.branches.get(branch) },
                main: false,
                current: false,
                locked: false,
                missing: false,
                switchTo: { worktree: path },
              })),
            ],
          });
          return true;
        case "run.list":
          reply({ runs: repo.runs.map((r, i) => ({ runId: `run-${i}`, ...r })) });
          return true;
        case "terminal.list":
          reply({
            terminals: repo.terminals.map((t, i) => ({
              terminalId: i,
              worktree: t.worktree,
              exited: t.state === "exited",
            })),
          });
          return true;
        case "git.send.theirs": {
          const theirs = [
            ...new Set([...repo.branches.values(), ...repo.incoming.values()]),
          ].sort();
          reply({ theirs, branchTip: repo.branches.get(params.branch as string) ?? null });
          return true;
        }
        case "git.send.prepare": {
          const tip = params.commit as string;
          if (!repo.commits.has(tip)) {
            fail(error(-32602, "no such commit"));
            return true;
          }
          const have = new Set(
            ((params.theirs as string[] | undefined) ?? []).flatMap((t) =>
              repo.commits.has(t) ? ancestors(repo, t) : [],
            ),
          );
          const chain = ancestors(repo, tip)
            .filter((c) => !have.has(c))
            .map((c) => [c, repo.commits.get(c) ?? null]);
          const bytes = btoa(JSON.stringify({ tip, chain }));
          const streamId = nextStream();
          reply({ streamId });
          const items: object[] = [
            { kind: "info", size: bytes.length, uncommitted: repo.dirty.length > 0, excluded: [] },
          ];
          for (let at = 0; at < bytes.length; at += repo.chunk)
            items.push({ kind: "data", offset: at, data: bytes.slice(at, at + repo.chunk) });
          // 실제 스트림처럼 다음 틱에 흘린다.
          setTimeout(() => {
            for (const data of items)
              conn.send({ method: "stream.chunk", params: { streamId, data } });
            conn.send({ method: "stream.end", params: { streamId, status: "completed" } });
          }, 0);
          return true;
        }
        case "git.send.receive.begin": {
          // crates/pw-git/src/send 의 머신 이름 규칙.
          const source = String(params.sourceMachine);
          if (
            !/^[A-Za-z0-9._-]{1,128}$/.test(source) ||
            source.startsWith(".") ||
            source.endsWith(".lock")
          ) {
            fail(error(-32602, "bad source machine name"));
            return true;
          }
          const uploadId = nextUpload++;
          uploads.set(uploadId, {
            owner: conn,
            branch: params.branch as string,
            commit: params.commit as string,
            sourceMachine: params.sourceMachine as string,
            size: params.size as number,
            data: "",
          });
          reply({ uploadId });
          return true;
        }
        case "git.send.receive.chunk": {
          const u = uploads.get(params.uploadId as number);
          if (!u || u.owner !== conn) {
            fail(error(-32602, "no such upload"));
            return true;
          }
          if (params.offset !== u.data.length) {
            uploads.delete(params.uploadId as number);
            fail(error(-32602, "chunk out of order"));
            return true;
          }
          u.data += params.data as string;
          reply({ received: u.data.length });
          return true;
        }
        case "git.send.receive.finish": {
          const u = uploads.get(params.uploadId as number);
          if (!u || u.owner !== conn) {
            fail(error(-32602, "no such upload"));
            return true;
          }
          uploads.delete(params.uploadId as number);
          const { tip, chain } = JSON.parse(atob(u.data)) as {
            tip: string;
            chain: [string, string | null][];
          };
          for (const [c, parent] of chain) repo.commits.set(c, parent);
          const ref = `refs/pitwall/incoming/${u.sourceMachine}/${u.branch}`;
          const previous = repo.incoming.get(ref) ?? null;
          repo.incoming.set(ref, tip);
          reply({ workTree: repo.workTree, incomingRef: ref, commit: tip, previous, pruned: [] });
          return true;
        }
        case "git.send.apply": {
          const branch = params.branch as string;
          const ref = `refs/pitwall/incoming/${String(params.sourceMachine)}/${branch}`;
          const done = (outcome: object) =>
            reply({ workTree: repo.workTree, incomingRef: ref, outcome });
          const stop = (reason: string, message: string | null) =>
            done({ kind: "stopped", reason, message });
          if (repo.incoming.get(ref) !== params.commit) {
            stop("incomingMoved", null);
            return true;
          }
          const tip = repo.branches.get(branch) ?? null;
          if (tip !== params.expected) {
            stop("branchMoved", null);
            return true;
          }
          const target = params.commit as string;
          if (tip !== null && !ancestors(repo, target).includes(tip)) {
            stop("diverged", "fatal: Not possible to fast-forward, aborting.");
            return true;
          }
          void ask(["src/main.rs"]).then((answer) => {
            if (answer === undefined)
              return fail(error(2006, "no project window is listening for git prompts"));
            if (answer.kind !== "proceed") return fail(error(-32002, "cancelled"));
            const switched = repo.head !== branch;
            repo.branches.set(branch, target);
            repo.head = branch;
            done({
              kind: "applied",
              previous: tip,
              current: target,
              created: tip === null,
              switched,
              lfsPointers: 0,
            });
          });
          return true;
        }
        case "git.resetPreview":
          reply({ lost: { changes: [], overwritten: [] }, fingerprint: "fp" });
          return true;
        case "git.reset": {
          // 실제처럼 준 것만 대조한다(`expected` · `expectedRef` 는 선택 칸 — 없으면 대조하지 않는다).
          const given = (v: unknown) => v !== undefined && v !== null;
          if (
            (given(params.expected) && params.expected !== tipOfHead()) ||
            (given(params.expectedRef) && params.expectedRef !== `refs/heads/${repo.head}`)
          ) {
            fail(error(-32004, "HEAD moved since it was read", { head: tipOfHead() }));
            return true;
          }
          repo.branches.set(repo.head, params.commit as string);
          reply({ head: params.commit });
          return true;
        }
      }
      return false;
    },
  };
  return host;
}
