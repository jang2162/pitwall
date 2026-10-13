# 공용 지침 관리

이 프로젝트의 지침 일부는 지침 서버에서 받은 사본이다.

- 사본은 직접 고치지 않는다: `.ruler/<스택>/NN-*.md`, `.ruler/<스택>/99-reference.md`, `docs/guideline/`, `guideline.json`·`guideline-lock.json`, 이 파일. 사본을 고쳐 원본에 올릴 것은 `guideline propose` 로, 새 지침은 `guideline-export` 스킬로 제안한다.
- 이 프로젝트에만 다르게 할 것은 바꿈에 쓴다. 자리는 `.ruler/<스택>/override.md`, 파일이 여럿이면 `.ruler/<스택>/override.d/*.md` 다. 바꿈 절마다 제목 바로 아래 첫 줄에 ``바꾸는 절: `<스택>/<파일>` 「<## 절 제목>」`` 을 쓰고(파일 전체를 바꾸면 「」 를 뺀다), 그 아래에 이 프로젝트의 규칙을 쓴다. 바꿈이 사본보다 우선한다.
- 지침을 최신으로 맞출 때는 `guideline status` 로 보고 `guideline update` 를 쓴다(에이전트는 `guideline-sync` 스킬).
- 프로젝트를 세팅할 때(그 스택을 처음 받았을 때, 새로 클론했을 때, 세팅이 맞는지 의심될 때)는 아래 문서를 읽고 따른다. 스택을 고른 것은 그 기술을 이 프로젝트에 들이기로 한 결정이다. 프로젝트가 아직 그 기술을 쓰지 않는다는 이유로 세팅을 건너뛰지 않는다 — 세팅이 바로 그 기술을 들이는 단계다. 세팅할 수 없으면(권한, 사람이 앱에서 해야 하는 설정 등) 건너뛰지 말고 사용자에게 알린다.

| 스택 | 세팅 문서 |
| --- | --- |
| base | `docs/guideline/base/SETUP.md` |
| codegraph | `docs/guideline/codegraph/SETUP.md` |
| mise | `docs/guideline/mise/SETUP.md` |
| orca | `docs/guideline/orca/SETUP.md` |
