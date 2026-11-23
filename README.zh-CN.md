[English](README.md) | [한국어](README.ko.md) | [日本語](README.ja.md) | **简体中文**

# bmo-cache-warmer

提示词缓存的保活调度器。提供方会在提示词前缀最后一次使用后缓存它一段时间,然后丢弃;对于即将再次使用的前缀,保持预热比
重新构建更省钱。预热器在不了解前缀内容的情况下让已注册的前缀保持预热:使用方为每个目标注册它所遵循的时钟,记录每一次
使用,并提供目标 ping 钩子;预热器负责运行轮次、判断哪些目标到期、请求其使用方去 ping 它们、掌管开关,并记录每一次 ping。
它不调用任何其他服务。除服务本身外,它还发布客户端、OpenAPI 文档、编码工具插件(一个 MCP 服务器和一个技能),以及一个
无依赖 crate 形式的调度规则。

## 目录结构

| 目录 | 内容 |
|---|---|
| `src/` | 服务包:服务、客户端、OpenAPI 文档和 MCP 服务器,以及二进制文件 |
| `schedule/` | 调度 crate:到期规则与计划,附带自己的测试 |
| `examples/` | 输出仓库所保存的 OpenAPI 文档的程序 |
| `plugin/` | 编码工具插件的技能 |
| `policy/` | 预热器自身的固定数值,是一个编译进二进制的 TOML 文件 |
| `tests/` | 仓库检查和服务测试 |
| `deploy/` | 服务的 systemd 用户单元,部署时从部署树链接 |
| `docs/` | 关于服务行为的权威文档,以及 OpenAPI 文档 |

根目录下有工作区的 Cargo 清单(同时也是服务包的清单)、版本文件、附带韩语台账的 `testbed.toml`、`install.toml`、`rustfmt.toml`、
`CLAUDE.md`,以及 `LICENSE` 中的 MIT 许可证。

## 文档

| 文档 | 说明 |
|---|---|
| `docs/api.md` | 边界:传输格式、路由、事件、目标 ping 钩子、错误码、客户端、调度 crate、编码工具插件、二进制的命令以及已部署的服务 |
| `docs/openapi.json` | 以 OpenAPI 3.1 文档形式表达的契约,由代码生成 |
| `docs/warming.md` | 轮次、目标何时到期、分组、每次应答留下什么、开关以及策略 |
| `docs/store.md` | 存储保存什么、其结构如何演变,以及如何生成、检查和恢复快照 |
| `docs/PERSONAL-DATA.md` | 仓库中任何文件都不得包含的个人数据、应改写成什么,以及读取每个文件来检查这一点的检查 |

## 版本

- **整个仓库只有一个版本,** 即工作区的版本,所有成员包都继承它。它记录在 `VERSION` 中,每个包的版本都与之相同。
- **改动如何影响版本**见 `docs/api.md` 的「Versioning」。

## 命令

```sh
cargo check --workspace --all-targets
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --example openapi > docs/openapi.json
```
