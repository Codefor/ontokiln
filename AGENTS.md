# OntoKiln 开发指南

本指南适用于整个仓库。OntoKiln 将一个目录的 Turtle（`.ttl`）和 N-Triples（`.nt`）文件编译成不可变 RDF 快照，再加载到内存提供只读推理。项目目标是亿级三元组。pack v2 已在单个合成图上验证 100,000,000 条去重显式三元组的编译、装载和在线查询。现有 CLI 和 HTTP 契约见 [README](README.md)，后续架构与验收目标见[设计稿](docs/rust-design-v1.md)。

## 按任务定位代码

| 要改什么 | 入口 | 需要保持的行为 |
|---|---|---|
| TTL 扫描、解析与编码 | [compiler](crates/compiler/src/lib.rs) 的 `compile_directory` | 稳定顺序扫描，按文档隔离空白节点，对词典排序、对三元组去重。同一输入可重建相同 pack |
| 离线文件格式与索引 | [pack](crates/pack/src/lib.rs) 的 `Pack::save/load` | 词项 ID 仅在当前快照有效。SPO 索引按主语、谓词、对象排序。保存时原子替换，加载时验证版本、摘要、长度和 ID 边界。只接受当前 pack v2，旧产物从源文件重新编译 |
| 层级推导与证明 | [reasoner](crates/reasoner/src/lib.rs) 的 `Reasoner::query` | 当前规则集为 `rdfs-hierarchy-v1`。按主语处理类和属性继承，显式事实优先，可返回一条证明。保留循环终止、工作量和结果大小限制 |
| CLI、启动、HTTP | [服务入口](src/main.rs) | `serve --ttl` 在启动时编译，`serve --pack` 可脱离 TTL 单独启动。二者共用同一内存推理器。除健康检查和单主语查询外，批量查询、类型交集及 SPARQL 模板的处理见 [queries](src/queries.rs) |
| 字段血缘和业务分支 | [scenario_lab](examples/scenario_lab.rs)、[场景说明](examples/scenarios/README.md) | 跨节点血缘、反向影响、覆盖与降级在独立实验程序中执行。Go 样例和 TTL 之间是手工映射，[用例](examples/scenarios/cases.json)给出独立预期。修改语义时核对确定集、候选集和被排除的路径 |

新增能力的分工与命令见[引擎说明](docs/engines.md)。可执行的规则定义由编译侧的 [rules](crates/compiler/src/rules.rs) 规划和离线求值。SPARQL 查询显式图，内存类型过滤使用按需构建的位图。修改 pack 时同时检查文件映射的生命周期、v2 偏移表和 SPO 的自然对齐和源文件被改写时的行为。

## 构建与验证

从仓库根目录执行。基准环境使用 Rust 1.93.1，Cargo 依赖版本由 `Cargo.lock` 固定。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo run --locked --example scenario_lab -- --compare
```

[引擎回归](tests/engines.rs)验证 mmap、并行确定性、SPARQL 和规则求值。使用 `--all-features` 覆盖 Ascent 与 Datafrog 的差分用例。[集成测试](tests/demo.rs)覆盖删除 TTL 后独立加载 pack、两种入口的答案与证明一致、循环继承、空白节点作用域及损坏包处理。业务语义改动可先用 `cargo run --locked --example scenario_lab -- --case V4 --trace` 查看具体选择过程，再重放全部用例。

## 复现 pack v2 性能基准

在 macOS 或 Linux 上使用 Python 3.9+。先构建带 Ascent 的 release 二进制，再从仓库根目录顺序运行两套脚本，避免争用 CPU。输出写到 Git 忽略的 `data/`。快速检查环境时，可给任一脚本加 `--subjects 25000`，参数表示生成的面板数。

```sh
cargo build --release --locked --features ascent-engine --bin ontokiln --example scenario_lab
python3 -B benchmarks/run.py --output data/pack-v2.json
python3 -B benchmarks/capabilities.py --output data/pack-v2-capabilities.json
```

| 改动涉及 | 运行与对照 |
|---|---|
| 编译、pack 装载、层级推导、证明、业务血缘或反向影响 | [常规脚本](benchmarks/run.py)覆盖三档数据规模及 14 个业务用例。与 [v2 常规基准](benchmarks/pack-v2.json) 比较 |
| 并行批量查询、位图类型过滤、SPARQL 或离线规则 | [扩展脚本](benchmarks/capabilities.py)覆盖首次与热态位图查询、两种规模的 SPARQL 聚合，以及 Datafrog / Ascent 闭包对照。与 [v2 扩展基准](benchmarks/pack-v2-capabilities.json) 比较 |

回归比较应匹配图规模、查询写法、并发、预热和构建特性。HTTP 往返、`scenario_lab` 进程内求值、离线规则 CLI 整条命令各自计时。比较批量吞吐时记录每请求主语数和主语/秒。位图首次建索引与热查询分别报告。

排查 SPARQL 聚合的 HTTP 422 时，先检查错误正文和候选行扫描预算。当前每请求累计允许检查 100,000,000 条候选行。历史 v2 报告按每请求 2,000 万行测量，比较不同运行时需记录各自的预算。修改求值器后重跑两种写法和小图对照，核对答案，并将预算拒绝耗时单列。[v2 报告及计时口径](benchmarks/pack-v2.md)

亿级基准使用同一套数据生成器，运行 `python3 -B benchmarks/hundred_million.py --output data/hundred-million.json`。保持 SPARQL 查询预算默认，记录 422 的尝试行号、限额及全扫描迭代器数，并核对位图结果。[亿级报告](benchmarks/hundred-million.md)

正式更新基准时，将实测 JSON 和报告一起提交，注明引擎提交、机器和 pack 版本。跨版本比较可查[历史基准](benchmarks/README.md)。单 pack 默认上限为 256 MiB，可通过进程环境变量 `ONTOKILN_MAX_PACK_MIB` 显式设置，扩大规模前先核算索引与词典占用。
