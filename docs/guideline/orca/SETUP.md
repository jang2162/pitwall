# orca 세팅

## 확인

- 저장소 루트에 `orca.yaml` 이 있고 주 체크아웃에 커밋돼 있다.
- Orca 앱의 저장소 설정에서 `orca.yaml` 을 쓰는 중으로 보이고, 스크립트 출처가 `orca.yaml` 을 포함한다.
- 워크트리를 새로 만들어 보면 런타임 버전과 의존성이 맞는다.
- `git check-ignore -q .orca/issue-command && echo ignored` 가 `ignored` 를 낸다.

## 세팅

1. 저장소 루트에 `orca.yaml` 이 없으면 만든다. 이미 있으면 키를 하나씩 `rules/orca/06-orca-yaml.md`(반영 후에는 `docs/guideline/orca/06-orca-yaml.md`)와 견준다.
2. `scripts.setup` 에 런타임을 맞추는 명령, 의존성 설치 명령, 로컬 설정 복사를 이 순서로 쓴다. 맨 위에 `set -e` 를 둔다.
3. 셋업이 에이전트의 작업 전제를 만들면 `setupAgentStartupPolicy: wait-for-setup` 을 쓴다.
4. 워크트리가 컨테이너·볼륨을 띄우면 `scripts.archive` 에 정리 명령을 쓴다.
5. `.gitignore` 에 `.orca/issue-command` 가 없으면 더한다. 개인용 이슈 명령 파일이라 커밋하지 않는다(`06-orca-yaml.md` 「스크립트 출처가 갈리는 것을 사람에게 알린다」). Ruler 블록(`# START Ruler Generated Files` … `# END Ruler Generated Files`)이 있으면 그 바깥에 쓴다.
   ```gitignore
   # Orca 개인용 이슈 명령
   /.orca/issue-command
   ```
   이미 추적 중이면(`git ls-files .orca/issue-command`) 사용자에게 보이고, 확인을 받아 `git rm --cached .orca/issue-command` 로 추적에서 뺀다.
6. 주 체크아웃 브랜치에 커밋한다.
7. 팀원에게 앱의 저장소 설정에서 스크립트 출처를 `orca.yaml` 로 맞추라고 알린다.
8. 워크트리를 하나 만들어 셋업이 돌고 결과가 맞는지 잰다.
