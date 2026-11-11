# OntoKiln 开发指南

OntoKiln 将指定目录的 Turtle / N-Triples 编译成不可变 RDF 快照，部署可只带 pack。当前仅接受 **pack v3**；升级 v1/v2 必须从源数据重编。v3 已验证一亿条去重显式三元组。[CLI 和 HTTP](README.md) · [查询、来源与证明](docs/query-service.md) · [早期设计与目标](docs/rust-design-v1.md)。

| 工作 | 入口 | 需要保持的行为 |
|---|---|---|
| 编译、源码版本 | [compiler](crates/compiler/src/lib.rs) | 顺序稳定地扫描文件；文档内隔离空白节点；排序、去重但保留所有断言来源。核对 sources.json 中的源码和 TTL 摘要。相同输入重建相同 pack |
| 离线索引 | [pack](crates/pack/src/lib.rs)、[候选区间](crates/pack/src/index.rs) | SPO 是规范行；POS/OSP 保存行号。所有 S/P/O 组合选最长前缀。写入临时文件后原子替换；加载时验证摘要、边界、排序及来源。词项 ID 不跨快照 |
| 视图与证据 | [reasoner](crates/reasoner/src/lib.rs)、[proofs](crates/reasoner/src/proofs.rs)、[离线规则](crates/compiler/src/rules.rs) | 版本先于推导；每个前提都需在所选版本可见。区分 asserted/inferred/closure；优先显式事实，一条确定性的有根证明。RDFS 规则版本 rdfs-hierarchy-v1；自定义规则版本含程序摘要 |
| SPARQL | [参数](crates/reasoner/src/parameters.rs)、[dataset](crates/reasoner/src/sparql.rs)、[spareval 补丁说明](vendor/spareval/ONTOKILN.md) | 显式参数槽绑定到 RDF 词项，保留变量作用域和解的重数。文件是具名图，默认图去重。不要将一般值相等改写为词项相等。算子剖析是可选项 |
| 在线生命周期 | [HTTP](src/queries.rs)、[调度](src/execution.rs)、[工作进程](src/worker.rs)、[堆计量](src/allocation.rs) | TTL / pack 共用已验证的私有只读快照；启动时预留一份 pack 的临时磁盘。工作进程继承服务快照 FD，不按原始路径重开文件。先终止并 wait，再释放槽。JSONL 以 end 记录标志完整结果 |
| 业务语义 | [版本化赋值链](examples/assignment/queries/assignment.rq)、[业务选择实验](examples/scenarios/README.md) | 来源为已有 RDF 声明，源码用于摘要核对。两个版本故意复用相同 IRI；结果不能跨版本。业务实验另有 14 个[独立预期](examples/scenarios/cases.json)，核对确定、候选、被排除的路径 |

服务目标平台为小端 macOS / Linux，测试还需 Python 3.9+。当前构建工具链固定为 Rust 1.95.0（历史基准保留原工具链记录），依赖由 Cargo.lock 固定。单 pack 默认 256 MiB，扩大前核算磁盘与内存，亿级基准设置 `ONTOKILN_MAX_PACK_MIB=4096`。规则引擎分工见[说明](docs/engines.md)。

## 构建与验证

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo run --locked --example scenario_lab -- --compare
```

[引擎回归](tests/engines.rs)覆盖 mmap、并行确定性、视图、证明导出重载及 Datafrog/Ascent 差分。[集成测试](tests/demo.rs)覆盖删除 TTL 后独立加载、循环、空白节点及损坏包。[真实 HTTP 回归](tests/query_service.py)由 Cargo 集成测试启动，验证版本、参数、算子统计、输出和堆上限、超时，以及首行前和流式传输中断连。不能用仅检查取消标志的模拟测试替代。修改业务选择逻辑，可先重放 `scenario_lab -- --case V4 --trace`，再比较全部用例。

## 预算与基准

CLI 和 HTTP 共用 SPARQL 核心。默认一次请求总共最多查看 **200,000,000 条候选索引行**；再次检查同一行再次计数，第 200,000,001 次触发 422。二分定位的键比较和中间解数量不计入扫描数。5 秒超时为 408，主动取消为 409，槽满为 429。流已经开始时，错误在末行。改预算时同步更新测试、README 和接口文档。堆增量、累计分配、共享 mmap、进程 RSS 分开报告。

```sh
cargo build --release --locked --features ascent-engine --bin ontokiln --example scenario_lab
python3 -B benchmarks/indexed_queries.py --output data/indexed-queries-v3.json
python3 -B benchmarks/worker_memory.py --output data/worker-memory-v3.json
```

[亿级 v3 报告](benchmarks/indexed-queries-v3.md)使用同一生成器：1,113,595 / 11,126,095 / 100,000,000 条显式事实。亿级双类型 COUNT 和 FILTER EXISTS 都得到 1,123,584；当前固定模板各读 6,741,500 条候选行。修改优化器后重测两种模板及小图对照；答案不能变化，扫描量应解释。每种模板预热一次、串行测九次，按状态分别报告成功、超时和拒绝。另一次 counters 请求用于记录扫描和堆；完整 profile 单列，避免计入延迟。内存测试区分父进程就绪、运行峰值、请求结束以及每个子进程的峰值；共享页不可简单相加。

常规编译、启动和层级实验运行 `benchmarks/run.py --output data/pack-v3.json`；批量、类型集合、SPARQL、离线规则运行 `benchmarks/capabilities.py --output data/pack-v3-capabilities.json`；均用 Python 3、按顺序跑，`--subjects 25000` 可快速检查。全量历史能力套件使用 `benchmarks/hundred_million.py`。v3 在线查询为每次请求新建工作进程，HTTP 类型位图不跨请求留存，不能把 v2 的热缓存成绩当作 v3。大型测试生成的 pack 不入库；正式报告应将实测 JSON 一起提交，记录引擎提交、构建特性、机器、预热和并发。[历史对照](benchmarks/README.md)
