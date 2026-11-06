# OntoKiln 开发指南

本指南适用于整个仓库。OntoKiln 将一个目录的 Turtle（`.ttl`）文件编译成不可变 RDF 快照，再加载到内存提供只读推理。项目目标是亿级三元组，当前 demo 的实测规模到 10,001,094 条。现有 CLI 和 HTTP 契约见 [README](README.md)，后续架构与验收目标见[设计稿](docs/rust-design-v1.md)。

## 按任务定位代码

| 要改什么 | 入口 | 需要保持的行为 |
|---|---|---|
| TTL 扫描、解析与编码 | [compiler](crates/compiler/src/lib.rs) 的 `compile_directory` | 稳定顺序扫描，按文档隔离空白节点，对词典排序、对三元组去重。同一输入可重建相同 pack |
| 离线文件格式与索引 | [pack](crates/pack/src/lib.rs) 的 `Pack::save/load` | 词项 ID 仅在当前快照有效。SPO 索引按主语、谓词、对象排序。保存时原子替换，加载时验证版本、摘要、长度和 ID 边界。变更格式时明确旧 pack 的读取或拒绝策略 |
| 层级推导与证明 | [reasoner](crates/reasoner/src/lib.rs) 的 `Reasoner::query` | 当前规则集为 `rdfs-hierarchy-v1`。按主语处理类和属性继承，显式事实优先，可返回一条证明。保留循环终止、工作量和结果大小限制 |
| CLI、启动、HTTP | [服务入口](src/main.rs) | `serve --ttl` 在启动时编译，`serve --pack` 可脱离 TTL 单独启动。二者共用同一内存推理器。HTTP 入口是 `GET /health` 和 `GET /v1/query` |
| 字段血缘和业务分支 | [scenario_lab](examples/scenario_lab.rs)、[场景说明](examples/scenarios/README.md) | 跨节点血缘、反向影响、覆盖与降级在独立实验程序中执行。Go 样例和 TTL 之间是手工映射，[用例](examples/scenarios/cases.json)给出独立预期。修改语义时核对确定集、候选集和被排除的路径 |

## 构建与验证

从仓库根目录执行。基准环境使用 Rust 1.93.1，Cargo 依赖版本由 `Cargo.lock` 固定。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked --example scenario_lab -- --compare
```

[集成测试](tests/demo.rs)覆盖删除 TTL 后独立加载 pack、两种入口的答案与证明一致、循环继承、空白节点作用域及损坏包处理。业务语义改动可先用 `cargo run --locked --example scenario_lab -- --case V4 --trace` 查看具体选择过程，再重放全部用例。

## 性能基准

[基准脚本](benchmarks/run.py)需要 macOS 或 Linux、Python 3.9+ 及 release 二进制。可先用较小数据验证环境。

```sh
cargo build --release --locked --bin ontokiln --example scenario_lab
python3 benchmarks/run.py --subjects 25000 --output data/performance-run.json
```

省略 `--subjects` 会运行全套规模。`--subjects` 指生成的面板数，实际三元组数由脚本通过服务健康检查核对。比较前确认数据规模、查询形状、并发和计时边界一致。HTTP 端到端延迟与 `scenario_lab` 进程内耗时分别报告。

日常复测输出放在 Git 忽略的 `data/`。正式更新基准时，将实测 JSON 与[性能报告](benchmarks/README.md)一起提交，注明环境和引擎版本。[已提交的基线](benchmarks/baseline.json)可用于对照。当前单 pack 上限是 256 MiB，扩大规模时先核算索引与词典占用。
