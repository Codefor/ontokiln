# OntoKiln 开发指南

## 仓库定位与边界

`Codefor/ontokiln` 是通用 RDF 编译与推理引擎。负责解析、编译 Turtle 等受支持的 RDF 格式，并根据显式输入执行规则推理、SPARQL 查询和 SHACL 校验；不负责设计本体或解释业务建模合同。输入格式、支持子集和执行限制以 [Full 合同](docs/full.md) 及当前实现为准。

| 所有者 | 职责 |
|---|---|
| 本仓库 | RDF 解析、快照与索引、通用查询和推理、标准约束校验、来源与证明、引擎身份、产物校验、执行预算与取消、通用构建入口 |
| 应用与知识资产仓库 | 本体建模、业务词汇与实体身份、优先级及选择策略、规则/Shapes/查询内容、私有数据转换、业务工作流、鉴权与租户、业务 API 和结果解释 |
| 环境适配与发行仓库 | 固定公开源码、内部源码镜像、工具链与构建平台适配、制品安装和分发；不维护另一套引擎或业务模型 |

### 核心不得感知业务建模

- `crates/*`、库与 CLI/HTTP 入口不得按业务类名、谓词、IRI 命名空间、字段名或业务 profile 执行专用分支。RDF/RDFS/SHACL 标准语义和已公开的通用规则 profile 不属于业务特判。
- 业务实体的 identity、优先级、字段赋值、实现选择和场景状态由调用方建模；核心只执行显式传入的事实、规则、查询与约束。不得在通用求值器中硬编码某个领域的答案或策略。
- 私有 JSON、源码结构或历史业务格式的解析与修复留在调用方。RDF 字面量不会因内容像 JSON 而被核心隐式解释；标准数据类型求值仍按支持的 RDF/SPARQL 语义执行。
- 引擎源码指纹、能力、格式版本和产物摘要属于通用引擎身份，由 [identity](src/identity.rs) 提供；它们与业务实体身份不同。按语义职责审查模块，不按 `identity.rs` 等文件名整体搬迁逻辑。
- 核心不依赖内部应用、构建平台或业务服务，不引入私有仓库路径、凭据和私有业务数据。通用缺陷在本仓库修复；环境适配留在发行仓库。

### 示例与变更评审

`examples/` 可保留公开、合成的领域数据及独立实验程序，用于演示显式 RDF/规则/查询的效果。示例中的建模和选择策略不成为核心默认行为，生产库、CLI 和 HTTP 不得依赖示例模块或自动加载示例规则；真实业务模型由调用方维护。

编辑前确认变更属于上表中的本仓库职责。若消费者需求缺少底层能力，应提炼为通用 RDF、查询、规则或执行合同，补齐核心能力及测试；不能以禁止业务逻辑为由遗漏必需的通用实现。可声明的领域差异通过调用方输入表达，不增加面向单个业务的配置开关。

涉及语义或扩展入口的变更，至少用合成输入证明：更换非标准业务词汇并同步更换规则/查询后结果仍按相同机制产生；未知业务名称或私有字面量不会触发额外行为。不支持的输入明确失败，不静默回退旧引擎。预算、取消、来源与实际证明覆盖仍是验收要求，不能因移除业务逻辑而放松。

## 模块与行为约束

OntoKiln 将指定目录的 Turtle / N-Triples 编译成不可变 RDF 快照，部署可只带 pack。当前仅接受 **pack v3**；升级 v1/v2 必须从源数据重编。v3 已验证一亿条去重显式三元组。[CLI 和 HTTP](README.md) · [查询、来源与证明](docs/query-service.md) · [早期设计与目标](docs/rust-design-v1.md)。

| 工作 | 入口 | 需要保持的行为 |
|---|---|---|
| 编译、源码版本 | [compiler](crates/compiler/src/lib.rs) | 顺序稳定地扫描文件；文档内隔离空白节点；排序、去重但保留所有断言来源。核对 sources.json 中的源码和 TTL 摘要。相同输入重建相同 pack |
| 离线索引 | [pack](crates/pack/src/lib.rs)、[候选区间](crates/pack/src/index.rs) | SPO 是规范行；POS/OSP 保存行号。所有 S/P/O 组合选最长前缀。写入临时文件后原子替换；加载时验证摘要、边界、排序及来源。词项 ID 不跨快照 |
| 视图与证据 | [reasoner](crates/reasoner/src/lib.rs)、[proofs](crates/reasoner/src/proofs.rs)、[离线规则](crates/compiler/src/rules.rs) | 版本先于推导；每个前提都需在所选版本可见。区分 asserted/inferred/closure；优先显式事实，一条确定性的有根证明。RDFS 规则版本 rdfs-hierarchy-v1；自定义规则版本含程序摘要 |
| SPARQL | [参数](crates/reasoner/src/parameters.rs)、[dataset](crates/reasoner/src/sparql.rs)、[spareval 补丁说明](vendor/spareval/ONTOKILN.md) | 显式参数槽绑定到 RDF 词项，保留变量作用域和解的重数。文件是具名图，默认图去重。不要将一般值相等改写为词项相等。算子剖析是可选项 |
| 在线生命周期 | [HTTP](src/queries.rs)、[调度](src/execution.rs)、[工作进程](src/worker.rs)、[堆计量](src/allocation.rs) | TTL / pack 共用已验证的私有只读快照；启动时预留一份 pack 的临时磁盘。工作进程继承服务快照 FD，不按原始路径重开文件。先终止并 wait，再释放槽。JSONL 以 end 记录标志完整结果 |
| Full 与 SHACL | [full](src/full.rs)、[full_rules](src/full_rules.rs)、[disk](src/disk.rs)、[shacl](src/shacl.rs)、[rdf_graph](src/rdf_graph.rs) | SQLite 编译、规则物化和查询；约束按显式 Shapes 求值。调用方选择输入、Shapes 与 focus，核心不按业务词汇分派 |
| Full 执行与身份 | [full_artifact](src/full_artifact.rs)、[full_execution](src/full_execution.rs)、[identity](src/identity.rs)、[构建入口](scripts/build.py) | 冻结输入、摘要核验、原子产物、进程预算、取消与回收；区分引擎与发行来源。Full 的 closure 与执行摘要不冒充逐事实 Proof |
| Full 常驻实例 | [session](src/session.rs)、[full_snapshot](src/full_snapshot.rs)、[会话协议](docs/session-v1.md) | 启动时核验并固定匿名只读数据库；worker 继承 FD，不重新打开源路径。单次 API 保持兼容；挂载磁盘与请求 scratch 分开；每请求仍先终止并 wait，取消后结束实例 |
| 独立领域示例 | [版本化赋值链](examples/assignment/queries/assignment.rq)、[业务选择实验](examples/scenarios/README.md) | 仅验证显式建模，不定义核心隐式语义。来源为已有 RDF 声明，源码用于摘要核对。两个版本故意复用相同 IRI；结果不能跨版本。实验另有 14 个[独立预期](examples/scenarios/cases.json)，核对确定、候选、被排除的路径 |

服务目标平台为小端 macOS / Linux，测试还需 Python 3.9+。当前构建工具链固定为 Rust 1.95.0（历史基准保留原工具链记录），依赖由 Cargo.lock 固定。单 pack 默认 256 MiB，扩大前核算磁盘与内存，亿级基准设置 `ONTOKILN_MAX_PACK_MIB=4096`。规则引擎分工见[说明](docs/engines.md)。

## 构建与验证

纯文档改动核对职责、模块链接及命令，并执行 `git diff --check`；无需重跑 Rust 构建或容量基准。实现改动按影响执行以下检查及相关 [Full 边界测试](tests/full.py)，不能用文档自查代替行为验证。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo run --locked --example scenario_lab -- --compare
```

[引擎回归](tests/engines.rs)覆盖 mmap、并行确定性、视图、证明导出重载及 Datafrog/Ascent 差分。[集成测试](tests/demo.rs)覆盖删除 TTL 后独立加载、循环、空白节点及损坏包。[真实 HTTP 回归](tests/query_service.py)由 Cargo 集成测试启动，验证版本、参数、算子统计、输出和堆上限、超时，以及首行前和流式传输中断连。不能用仅检查取消标志的模拟测试替代。修改独立示例中的选择策略或其依赖的通用引擎行为，可先重放 `scenario_lab -- --case V4 --trace`，再比较全部用例。

## 预算与基准

CLI 和 HTTP 共用 SPARQL 核心。默认一次请求总共最多查看 **200,000,000 条候选索引行**；再次检查同一行再次计数，第 200,000,001 次触发 422。二分定位的键比较和中间解数量不计入扫描数。5 秒超时为 408，主动取消为 409，槽满为 429。流已经开始时，错误在末行。改预算时同步更新测试、README 和接口文档。堆增量、累计分配、共享 mmap、进程 RSS 分开报告。

```sh
cargo build --release --locked --features ascent-engine --bin ontokiln --example scenario_lab
python3 -B benchmarks/indexed_queries.py --output data/indexed-queries-v3.json
python3 -B benchmarks/worker_memory.py --output data/worker-memory-v3.json
```

[亿级 v3 报告](benchmarks/indexed-queries-v3.md)使用同一生成器：1,113,595 / 11,126,095 / 100,000,000 条显式事实。亿级双类型 COUNT 和 FILTER EXISTS 都得到 1,123,584；当前固定模板各读 6,741,500 条候选行。修改优化器后重测两种模板及小图对照；答案不能变化，扫描量应解释。每种模板预热一次、串行测九次，按状态分别报告成功、超时和拒绝。另一次 counters 请求用于记录扫描和堆；完整 profile 单列，避免计入延迟。内存测试区分父进程就绪、运行峰值、请求结束以及每个子进程的峰值；共享页不可简单相加。

常规编译、启动和层级实验运行 `benchmarks/run.py --output data/pack-v3.json`；批量、类型集合、SPARQL、离线规则运行 `benchmarks/capabilities.py --output data/pack-v3-capabilities.json`；均用 Python 3、按顺序跑，`--subjects 25000` 可快速检查。全量历史能力套件使用 `benchmarks/hundred_million.py`。v3 在线查询为每次请求新建工作进程，HTTP 类型位图不跨请求留存，不能把 v2 的热缓存成绩当作 v3。大型测试生成的 pack 不入库；正式报告应将实测 JSON 一起提交，记录引擎提交、构建特性、机器、预热和并发。[历史对照](benchmarks/README.md)
