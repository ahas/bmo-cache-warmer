[English](README.md) | **한국어** | [日本語](README.ja.md) | [简体中文](README.zh-CN.md)

# bmo-cache-warmer

프롬프트 캐시를 위한 keepalive 스케줄러입니다. 제공자는 프롬프트 접두부를 마지막 사용 후 한동안 캐시해 두었다가
버립니다. 곧 다시 쓰일 접두부라면 새로 만드는 것보다 따뜻하게 유지하는 편이 저렴합니다. 워머는 등록된 접두부가 무엇인지
모른 채로 그것들을 따뜻하게 유지합니다. 사용하는 쪽은 각 대상을 그것이 따르는 시계와 함께 등록하고, 사용할 때마다 기록하며,
대상 핑 훅을 제공합니다. 워머는 패스를 실행하고, 어떤 대상이 기한이 되었는지 판단해 그 사용자에게 핑을 요청하며, 스위치를
쥐고 모든 핑의 기록을 남깁니다. 다른 서비스는 호출하지 않습니다. 서비스와 함께 클라이언트, OpenAPI 문서, 하네스
플러그인(MCP 서버와 스킬), 그리고 의존성이 없는 크레이트로 된 스케줄 규칙을 공개합니다.

## 구성

| 폴더 | 내용 |
|---|---|
| `src/` | 서비스 패키지: 서비스, 클라이언트, OpenAPI 문서와 MCP 서버, 바이너리 |
| `schedule/` | 스케줄 크레이트: 기한 규칙과 계획, 그리고 자체 테스트 |
| `examples/` | 저장소가 보관하는 OpenAPI 문서를 출력하는 프로그램 |
| `plugin/` | 하네스 플러그인의 스킬 |
| `policy/` | 워머 고유의 고정 수치. 바이너리에 내장되는 TOML 파일입니다 |
| `tests/` | 저장소 검사와 서비스 테스트 |
| `deploy/` | 서비스의 systemd 사용자 유닛. 배포 시 배포 트리에서 링크됩니다 |
| `docs/` | 서비스가 하는 일에 대한 정본 문서와 OpenAPI 문서 |

루트에는 서비스 패키지의 매니페스트이기도 한 워크스페이스 Cargo 매니페스트, 버전, 한국어 대장을 갖춘 `testbed.toml`,
`install.toml`, `rustfmt.toml`, `CLAUDE.md`, 그리고 `LICENSE`의 MIT 라이선스가 있습니다.

## 문서

| 문서 | 내용 |
|---|---|
| `docs/api.md` | 경계: 와이어 형식, 라우트, 이벤트, 대상 핑 훅, 코드, 클라이언트, 스케줄 크레이트, 하네스 플러그인, 바이너리 명령, 배포된 서비스 |
| `docs/openapi.json` | 코드에서 작성한 OpenAPI 3.1 문서 형태의 계약 |
| `docs/warming.md` | 패스, 대상의 기한이 되는 시점, 그룹, 각 응답이 남기는 것, 스위치, 정책 |
| `docs/store.md` | 저장소가 보관하는 것, 그 형태가 바뀌는 방식, 스냅샷을 만들고 확인하고 되돌리는 방법 |
| `docs/PERSONAL-DATA.md` | 저장소의 어떤 파일에도 담아서는 안 되는 개인 정보, 그 대신 들어갈 것, 모든 파일을 읽어 이를 확인하는 검사 |

## 버전

- **저장소의 버전은 하나이며,** 워크스페이스의 버전이고 모든 멤버 패키지가 이를 상속합니다. `VERSION`에 기록되며 각
  패키지의 버전은 이와 같습니다.
- **변경이 버전을 어떻게 바꾸는지**는 `docs/api.md`의 「Versioning」에 적혀 있습니다.

## 명령

```sh
cargo check --workspace --all-targets
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --example openapi > docs/openapi.json
```
