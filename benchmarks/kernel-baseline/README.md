# API 契约复核与编译、推理基线（2026-11-13）

**结论：修复本轮发现的契约缺口后，可以以 `ontokiln-api/v1` JSON 协议为稳定边界开展内核性能优化。被测 `46e0fb5` 的 Full 编译与有界规则物化已完成百万条合成数据测量；该版本的全图查询和 SHACL 有明确的访问路径与内存瓶颈。**

本报告固定记录 `46e0fb5` 的 512 MiB 优化前基线。同一内核后续的 [4 GiB 百万/千万编译、物化实测及亿级推演](46e0fb5-scaling.md) 单独报告，两种预算不能拼接为一条性能曲线。在测量期间，PR 的并行提交 [`1f1f29a`](https://github.com/Codefor/ontokiln/commit/1f1f29a0b4f9b5b86ef1eab93defc68654568d33) 增加了常驻 Full 快照与 SHACL 导航索引；后续的 SPO 索引与 CONSTRUCT 准入修复也不在本报告二进制中；并行工作的 [10K/100K 优化对照](../kernel-20261113.md) 独立保留。正式结果为 `COMPLETE_WITH_FAILURES`，失败和跳过都保留；这不是一次“所有百万级能力均已通过”的容量认证。

- [逐样本原始 JSON](46e0fb5-linux.json)
- [release 构建记录](46e0fb5-linux-build.json)
- [测量脚本](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/benchmarks/kernel_baseline.py)、[比较工具](../compare_kernel_baseline.py)、[生成器与答案校验说明](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/tests/benchmarks/README.md)
- [API v1 契约](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/docs/api-v1.md)、[Full 语义与支持边界](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/docs/full.md)

## 1. 这轮契约复核的结果

契约修复提交为 [`7082997`](https://github.com/Codefor/ontokiln/commit/70829970e197e9b897207c6516881bf354d0e784)，基准工具与测量源码提交为 [`46e0fb5`](https://github.com/Codefor/ontokiln/commit/46e0fb5b6089e9f5bacb8494437c956e21e936a2)。本报告随后提交，报告所在 HEAD 不应被误认为被测二进制的源码提交。

| 发现 | 本次处理 | 对调用方的影响 |
|---|---|---|
| Full 的 `content_digest` 覆盖文件 inventory，不包含外层 manifest；仅固定它不足以固定 kind/profile/fixpoint 等语义元数据 | 查询和校验增加可选 `expected_manifest_sha256`；核对实际读取并验证的原始 manifest 字节；同时交叉校验冻结输入、profile 和语义指纹 | 旧请求省略或传 null 保持原行为；新调用方协商 `dataset-manifest-digest/v1`，使用 compile/materialize 返回的两个摘要固定完整快照 |
| 规则候选行预算在 `u64::MAX` / `i64::MAX` 附近可能溢出或变为无效 SQL LIMIT | 饱和加一并约束到 SQLite 可接受范围；保留真正预算不足时的资源错误 | 大预算不再造成 panic 或 datatype mismatch；零预算、边界预算和推导答案有回归覆盖 |
| `ASK` 内只在 FILTER 中出现的初始 RDF binding 未正确暴露 | 与 SELECT 共用绑定暴露机制，同时保留 ASK 的布尔响应形式 | `ASK { FILTER(BOUND(?given)) }` 等请求按显式绑定求值 |
| 未使用的 VALUES 表里，无效 RDF 词项可能逃过校验 | 绑定时先校验全部非空单元格 | 无效 IRI、语言与 datatype 冲突等统一失败；有效未使用表与 UNDEF 仍允许 |

稳定范围包括版本协商、默认值、请求/响应形状、RDF 词项、解的重数、声明的排序、视图、预算/取消与错误类别。能力列表允许增加成员，调用方按集合协商；不按能力列表完整字节串判断兼容性。

本次也明确了后续优化必须保持的语义：SPARQL 默认图是跨图去重 union；内置 RDFS 和三元组形式的规则 body/head 默认使用物理默认图 `g=0`，四项显式规则才指定图。无完整 ORDER BY 的结果切片、排序并列、非确定函数等按 SPARQL 允许的变化判断等价，不能把任意一次输出顺序冻结为通用优化约束。

**本次被测版本的边界。** 原生 Rust 接口/ABI 不属于这个 JSON 稳定承诺。本次 API 产物使用 `kernel-manifest-v1` 与 `ontokiln-sqlite-read/v1`，读取兼容性按格式/语义合同和完整性检查判断；另行兼容的旧 `kernel-manifest-v2` 产物要求精确 kernel identity，不能泛化为所有 Full 产物都按二进制 hash 锁定。升级二进制也不会给既有不可变数据库补充新索引，性能对照应各自重新编译。被测 `46e0fb5` 的 SHACL 并非完整标准覆盖：隐式 class target 仍需显式 `sh:targetClass`，`sh:sparql` 报告未完整投影每行 value/path，原生 Core 报告没有统一的 constraint-component 字段。这些是已披露的覆盖缺口，未来应独立进行语义变更评审与回归，不能把缺口当成永久合同，也不能混入纯性能改动。

发布前另复核 PR 最新 `5ad597f`：文件 API 继续使用原 `ontokiln-api/v1` 请求/响应边界，常驻入口使用独立 `ontokiln-session/v1`。后续 CONSTRUCT 修复允许本地 CONSTRUCT `.rq` 资产准入，同时让 SELECT/ASK 与 CONSTRUCT 统一经过本地查询检查，SERVICE/FROM 等外部访问会在准入阶段明确拒绝；这属于准入行为收紧，不能概括为所有旧的可解析查询都原样接受。顶层 `query` 的 SELECT/ASK 结果合同未扩成 CONSTRUCT。最新版本仍要求显式 SHACL targets/focus，未承诺隐式 class target。匿名快照的磁盘预留还会影响 peak scratch 的解释，旧新版本的资源指标需结合相应定义阅读。上述复核不把新提交当作本报告的被测内核。

## 2. 被测构建与环境

| 项目 | 记录 |
|---|---|
| 源码提交 | `46e0fb5b6089e9f5bacb8494437c956e21e936a2`，开始测量时工作树干净 |
| 源码树 | `f87335ca4a97b0290f56cae4185fc63c9e714cc7` |
| 构建 | Rust 1.95.0，release，default features，Cargo.lock 固定依赖；构建与测试在计时之前完成 |
| kernel SHA-256 | `af47e93d7adf7ccb70d1fbf69bd7e8f61467e4c2a339b85d2d5a99924d9acf2b` |
| 二进制 SHA-256 | `1fb31975bc3f81980e5c89650364c1fad56d6b5f75b59dc5bef21c900faefc0c` |
| 平台 | Linux 6.18.57，x86_64，glibc 2.41，Python 3.12.14 |
| CPU / 内存 | 可见 CPU 型号 AMD EPYC 9V74；affinity 9 个 vCPU；可见物理内存约 9.73 GiB；无可见 swap |
| 文件系统 | 工作目录与子进程 scratch 均在 `/workspace` 的 overlay 文件系统；底层物理设备未识别 |
| 请求执行 | 串行；Rayon 1 线程、Tokio 1 worker；每请求新进程，不是常驻服务热缓存 |
| 每次请求预算 | 60 s，API combined RSS 512 MiB，scratch 4 GiB；100 轮，最多 1,000,000 新事实、10,000,000 中间行 |
| 缓存 | 不清 OS page cache；生成、摘要计算和编译在查询前，可能预热文件缓存 |
| 时间 | 2026-11-13 03:22:51.962–03:35:48.610 UTC |

cgroup 限额文件不可见的值在原始记录中保留 null，不能解读为资源无限。9 个可见 vCPU 也不意味着每项测量使用了 9 核。这是当前共享 Linux 环境的基线，不能直接与仓库历史 M2 Pro/arm64、HTTP 常驻服务数据计算加速比。

`end_to_end_seconds` 从创建命令进程前计到退出并读取响应字节；Python JSON 解析与答案核对不在其中。Full API 时间还包含摘要验证、私有 SQLite 副本与发布等成本。查询/SHACL 尚无独立核心阶段计时，不通过相减推导“纯内核耗时”。`engine_seconds` 与 compile 的 parse/index 阶段重叠，不得再次相加。每项成功请求保留原始样本、最小值、最大值与中位数；3/9 个样本不支持 p95/p99 或统计显著性结论。

`peak_rss_bytes` 是 API 预算使用的 supervisor/worker combined RSS 指标，不是 PSS；`wait4` 的命令高水位另记，不能与它相加。`peak_scratch_bytes` 是采样的逻辑字节峰值，短请求可能漏掉瞬时副本；并不等于完整输出大小或绝对磁盘峰值。legacy pack compile 仅受外层超时监管，没有 Full 的 RSS/scratch 预算执法，不能把两种 compile 的资源约束视为相同。

## 3. 数据、规则与答案校验

每档由 10 条 schema 和每条订单/客户记录的 10 条事实组成；使用相同的 N-Triples 字节作为 pack 与 Full 的 asserted 数据。Full 另外编译规则、冻结输入与 plan，输出职责比 legacy pack compile 多，因此两项编译耗时并非同等工作量的实现竞赛。

| 显式事实 | 订单数（客户数相同） | N-Triples MiB | 预期新增事实 | 预期物化后事实 |
|---|---|---|---|---|
| 10,000 | 999 | 1.146 | 8,615 | 18,615 |
| 100,000 | 9,999 | 11.467 | 86,240 | 186,240 |
| 1,000,000 | 99,999 | 114.764 | 862,490 | 1,862,490 |


使用 `rdfs-supported-v1` 和三条显式正规则：next→reach、reach+next→reach、customer+region→inRegion。每组链最多 8 个节点，尾部 self-loop，闭包规模随输入线性增加；这不是任意长链、稠密关系或完整 OWL 的推理基准。三档均为 8 次 fixpoint 迭代、136 次规则求值；候选行分别为 18,607 / 186,232 / 1,862,482。候选计数不等于 SQLite 内部真实扫描行数。

生成器直接计算预期事实数、继承谓词数、所有分组 COUNT/SUM、固定完整排序的业务结果、属性路径和 SHACL focus 集。它不从引擎输出学习答案。查询还核对 RDF 类型、字面量词法、语言/数据类型和重数；两个 all-order Shapes 使用相同约束和相同 focus 集，仅 targetSubjectsOf/targetClass 不同；minCount 负例防止跳过 Core 校验被误认为通过。每个 Full 读取请求都固定 manifest 和 content 双摘要。

百万级 compile/materialize 的返回计数符合独立计算的预期，定向 closure 业务查询也通过；但百万级全量 inferred 谓词分组未能完成，所以不能声称该档全部推导事实已通过查询结果校验。计时结束后追加的 [直接 SQL 审计](46e0fb5-fact-audit.json) 已独立核对该档实际总量、逐谓词分布及确定性关系抽样；这不把 SPARQL 超时改判为性能通过，也不是对每条推导关系的完整证明。默认图合成输入之外，跨图语义依赖契约回归套件覆盖，不由本次吞吐基准证明。

## 4. 样本完整性

每档计划三次 pack compile、三次 Full compile、三次 materialize；每个查询预热一次、测九次，每个 SHACL 预热一次、测三次。某阶段首个请求失败后，跳过它的剩余重复，其他独立阶段继续。失败请求的耗时不进入成功中位数。

| 显式事实 | 完整成功阶段 | 成功测量样本 | 成功预热 | 实际失败预热 | 跳过测量样本 |
|---|---|---|---|---|---|
| 10,000 | 20/20 | 132 | 17 | 0 | 0 |
| 100,000 | 20/20 | 132 | 17 | 0 | 0 |
| 1,000,000 | 13/20 | 99 | 10 | 7 | 33 |


合计 363 个成功测量样本、44 个成功预热、7 个实际失败请求和 33 个跳过的计划请求。447 个计划请求中实际发起 414 个，407 个成功请求通过其对应答案校验。七次失败全部发生在百万级预热；不能表述成“每项九次超时”或“完成了失败项的性能采样”。

## 5. 全部阶段端到端中位数

单位均为秒；成功样本数量见上节。失败项没有成功延迟中位数，具体终止时间单列在后面。

| 阶段 | 10K | 100K | 1M |
|---|---|---|---|
| pack 编译 | 0.023 | 0.206 | 2.337 |
| Full 编译 | 0.094 | 0.751 | 8.481 |
| Full 规则物化 | 0.105 | 1.063 | 11.383 |
| pack / 点查 | 0.015 | 0.028 | 0.106 |
| pack / 客户区域连接 | 0.018 | 0.027 | 0.127 |
| pack / 区域聚合 | 0.016 | 0.045 | 0.327 |
| pack / 有界属性路径 | 0.014 | 0.026 | 0.106 |
| pack / 显式事实 COUNT | 0.014 | 0.046 | 0.319 |
| Full compiled / 点查 | 0.014 | 0.068 | 0.320 |
| Full compiled / 客户区域连接 | 0.025 | 0.163 | 1.605 |
| Full compiled / 区域聚合 | 0.112 | 0.894 | 25.193 |
| Full compiled / 有界属性路径 | 0.015 | 0.046 | 0.319 |
| Full compiled / 显式事实 COUNT | 0.065 | 4.579 | 预算耗尽；无成功测量样本 |
| Full reasoned / inferred 谓词分组 | 0.057 | 4.051 | 预算耗尽；无成功测量样本 |
| Full reasoned / closure 业务连接 | 0.015 | 0.055 | 0.381 |
| SHACL / targetSubjectsOf 全订单 | 0.231 | 2.202 | 预算耗尽；无成功测量样本 |
| SHACL / targetClass 全订单 | 0.351 | 31.379 | 预算耗尽；无成功测量样本 |
| SHACL / 单 focus minCount 负例 | 0.127 | 1.409 | 预算耗尽；无成功测量样本 |
| SHACL / 编译态 hasValue 负例 | 0.066 | 0.842 | 预算耗尽；无成功测量样本 |
| SHACL / 推理态 hasValue 正例 | 0.116 | 1.675 | 预算耗尽；无成功测量样本 |


`region_aggregate` 核对完整分组数与数值 SUM；业务连接核对完整 ORDER BY 下的固定前 16 个结果；点查和有界路径的答案规模固定。这些工作量不同，不能把它们的每请求延迟直接等同为“每条 RDF 的查询速度”。

## 6. 编译与物化的阶段成本、资源和产物

阶段时间取三个成功样本的中位数；RSS/scratch 取三个已记录峰值中的最大值，均排除预热。parse/index 各自中位数之和未必恰等于 engine 的中位数。

| 事实数 | 操作 | E2E 范围 s | 阶段中位数 s | 最大记录 RSS MiB | 最大记录 scratch MiB |
|---|---|---|---|---|---|
| 10,000 | Full 编译 | 0.093–0.095 | parse 0.030 / index 0.034 / engine 0.066 | 17.83 | 2.37 |
| 10,000 | Full 规则物化 | 0.104–0.126 | reasoning 0.073 | 17.71 | 2.69 |
| 100,000 | Full 编译 | 0.719–0.779 | parse 0.279 / index 0.317 / engine 0.609 | 41.90 | 23.41 |
| 100,000 | Full 规则物化 | 0.935–1.083 | reasoning 0.840 | 47.97 | 27.83 |
| 1,000,000 | Full 编译 | 8.030–8.662 | parse 3.271 / index 4.024 / engine 7.262 | 165.68 | 344.91 |
| 1,000,000 | Full 规则物化 | 11.014–11.497 | reasoning 9.425 | 147.05 | 426.37 |


| 显式事实 | pack MiB | compiled SQLite MiB | compiled 总产物 MiB | reasoned SQLite MiB | reasoned 总产物 MiB |
|---|---|---|---|---|---|
| 10,000 | 0.376 | 1.211 | 2.361 | 1.535 | 2.685 |
| 100,000 | 3.706 | 11.434 | 22.905 | 14.926 | 26.397 |
| 1,000,000 | 37.095 | 119.422 | 234.190 | 156.930 | 271.698 |


产物表取每阶段第一个成功输出的逻辑大小；Full 总产物包含冻结的 RDF 源、规则与 manifest/plan，不能仅与 pack 索引文件体积比较。原始 JSON 另有逐文件逻辑/分配大小。当前 Full compile 每档扩大十倍时耗时约增 8.0 / 11.3 倍，物化约增 10.1 / 10.7 倍；这是该有界闭包负载下的趋势，不是一般推理复杂度保证。百万级 parse/dictionary 约 3.27 s、后续 index/plan/维护约 4.02 s，后续编译优化需要同时观察两段。

## 7. 百万级失败与 SHACL 内存基线

API 对下面请求均返回 `RESOURCE_EXHAUSTED` / `BUDGET_EXHAUSTED`，没有有效完整结果；外层 watchdog 未触发。API 文本统一描述 time/RSS/scratch 预算，右列是根据实际时间、RSS 和 scratch 对触发预算的归因。

| 阶段（均为首次预热） | 终止前 E2E s | 记录 RSS MiB | 记录 scratch MiB | 预算归因 |
|---|---|---|---|---|
| Full compiled / 显式事实 COUNT | 60.041 | 22.08 | 132.56 | 约 60 s；RSS/scratch 远低于上限，指向时间预算 |
| Full reasoned / inferred 谓词分组 | 60.041 | 22.58 | 169.03 | 约 60 s；RSS/scratch 远低于上限，指向时间预算 |
| SHACL / targetSubjectsOf 全订单 | 6.525 | 514.37 | 249.32 | RSS 超过 512 MiB；时间与 scratch 未到上限 |
| SHACL / targetClass 全订单 | 5.607 | 512.81 | 247.87 | RSS 超过 512 MiB；时间与 scratch 未到上限 |
| SHACL / 单 focus minCount 负例 | 4.982 | 515.45 | 248.03 | RSS 超过 512 MiB；时间与 scratch 未到上限 |
| SHACL / 编译态 hasValue 负例 | 5.066 | 513.38 | 248.80 | RSS 超过 512 MiB；时间与 scratch 未到上限 |
| SHACL / 推理态 hasValue 正例 | 7.976 | 512.68 | 410.76 | RSS 超过 512 MiB；时间与 scratch 未到上限 |


监控按间隔采样，终止时 RSS 略超过 512 MiB 属于本次实际记录；这些终止耗时不是成功校验延迟，截断时 RSS 也不能用于推算完成校验所需的内存。


| 成功 SHACL 阶段 | 10K 最大记录 RSS MiB | 100K 最大记录 RSS MiB |
|---|---|---|
| SHACL / targetSubjectsOf 全订单 | 44.73 | 280.94 |
| SHACL / targetClass 全订单 | 44.73 | 281.77 |
| SHACL / 单 focus minCount 负例 | 42.35 | 262.86 |
| SHACL / 编译态 hasValue 负例 | 30.85 | 150.28 |
| SHACL / 推理态 hasValue 正例 | 42.17 | 262.98 |


100K 的两个 all-order Shapes 都检查精确的 9,999 个订单，targetSubjectsOf 约 2.20 s，targetClass 约 31.38 s，内存接近。单 focus 的 minCount 负例仍约 1.41 s / 262.86 MiB；百万级同样触发 512 MiB。这说明目标范围缩小没有消除现有整图装载与表示准备成本，不能把问题简单解释为校验结果条数太多。

## 8. 基线揭示的扩展风险（后续优化线索）

### Full 默认图扫描访问路径

10K→100K，显式 COUNT 从约 0.065 s 增至 4.579 s（约 70.3 倍），inferred 谓词分组从 0.057 s 增至 4.051 s（约 70.8 倍）；百万级两者均未在 60 s 内完成。相同显式数据的 pack COUNT 为 0.014 / 0.046 / 0.319 s。它们都逐请求启动，Full 的完整快照校验/复制也是其端到端成本的一部分。

[磁盘迭代器](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/src/disk.rs) 的默认图查询使用跨图 union，每 256 行重新执行 `DISTINCT`、SPO keyset 条件与 `ORDER BY s,p,o`；[当前索引](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/src/full.rs) 为主键 `(g,s,p,o)`、POS、OSP，缺少跨图全局 SPO 顺序。不同图的相同 SPO 必须去重，不能以增加 `g=0` 条件规避这条路径。

正式计时结束后，用 release 引擎链接的同一份 SQLite 3.54.1 静态库，以只读连接解释实际分页 SQL。compiled/asserted、reasoned/inferred、reasoned/closure 各取首、中、尾三个代表性 cursor；9 个计划全部包含：

```text
SEARCH facts USING PRIMARY KEY (ANY(g) AND (s,p,o)>(?,?,?))
USE TEMP B-TREE FOR DISTINCT
USE TEMP B-TREE FOR ORDER BY
```

这确认了主键范围访问之后，每次分页语句仍需临时去重与排序；不能称为无条件全表 SCAN，也尚未测得真实扫描行数、排序工作量或优化后的加速比。三个 cursor 是代表性键界限，不是执行 SELECT 取得的实际末行；工具只 step `EXPLAIN QUERY PLAN`，没有运行底层全图查询。

[执行计划原始证据](46e0fb5-sqlite-explain.json)记录运行时版本/source ID、静态库 SHA-256、编译命令、SQL/参数、计划行及同 disk.rs 的四项 PRAGMA。[最小 C 取证程序](sqlite_explain.c)可重新链接对应构建的 `libsqlite3.a`。两个历史 SQLite 文件的摘要、大小及时间戳在取证前后保持一致，manifest 的摘要也保持一致；这项检查在计时之后完成，不混入基准样本。

完成规模基线与推演后，一个可验证的单变量候选是增加覆盖此访问路径的 `facts(s,p,o,g,explicit)` 索引，先保持分页大小、COUNT 求值器、物化算法和公开协议不变。此索引在被测 `46e0fb5` 中尚未实施；后续并行提交的实现与收益见独立优化报告，不改变这里的测量结果。后续应同时比较 compile 的 index 阶段、materialize、产物大小、点查与 snapshot scratch；多一棵索引会增加构建、写入、VACUUM、摘要和每请求复制成本。不能只用 COUNT 加速判断整体收益。

### SHACL 目标发现与整图内存

[目标选择](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/src/shacl.rs) 的 targetClass 路径枚举所有 subject，并逐个调用 `has_type`；[Graph::objects](https://github.com/Codefor/ontokiln/blob/46e0fb5b6089e9f5bacb8494437c956e21e936a2/src/rdf_graph.rs) 当前对 BTreeSet 做整表过滤，类型与 subclass 检查因而重复扫描。与同 focus 集的 targetSubjectsOf 对照，100K 的 targetClass 慢约 14.3 倍，10K→100K 的 targetClass 耗时增约 89.5 倍。这是源代码与观测一致的热点证据，尚非 CPU profile 的占比结论。

先验证利用已有 SPO 排序做范围查找、共享目标类型索引或 subclass 闭包，保持继承语义、focus 集和报告结果等价。与此同时，当前验证会准备字符串 Graph、查询 Dataset 和原生验证表示，百万级内存预算失败要求减少重复整图表示或引入受预算约束的访问层。不能只把 targetClass 改快而忽略五项 SHACL 都无法在 512 MiB 内完成的事实。

### 编译与规则物化

现有有界递归已在百万级生成 862,490 个新增事实，8 轮收敛；先保留此基线，之后按 parse/dictionary、索引与发布、join 候选、去重和 SQLite 维护逐段优化。若改规则索引或物化批量写策略，每次都重新核对谓词分组、closure 业务连接和编译态/推理态 SHACL 对照，避免只看返回总数。

每次性能改动都需要同一输入、同一预算、同一计时方法的新报告和原始样本；新旧内核分别重新 compile，并使用各自产物返回的双摘要。跨图重复 SPO、跨 256 行分页边界、asserted/inferred/closure、overlay 和词项重数属于正确性门槛。不得原地给本次历史 artifact 增加索引再重写摘要。若必须改基准方法或预算，保留两套记录并明确标为不可直接比较，不把失败缩短解释为性能提升。

## 9. 复现与比较

先从本报告注明的干净源码提交单独构建；固定工具链由仓库提供。实际构建命令为：

```sh
ONTOKILN_BUILD_REVISION=46e0fb5b6089e9f5bacb8494437c956e21e936a2 \
RUSTFLAGS= CARGO_BUILD_JOBS=4 \
cargo build --release --locked --offline --bin ontokiln
```

实际构建时 `CARGO_ENCODED_RUSTFLAGS` 未设置；`--offline` 需要依赖已缓存，依赖下载应在计时之外完成。报告中的构建记录与二进制/kernel 摘要用于核对身份，不能仅凭路径名里的 `release` 认定构建方式。

```sh
python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln \
  --output data/kernel-baseline.json \
  --work-dir data/kernel-work \
  --sizes 10000 100000 1000000 \
  --build-samples 3 --query-samples 9 --validation-samples 3 \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 \
  --max-derived-facts 1000000 --max-intermediate-rows 10000000 \
  --max-iterations 100 --threads 1 \
  --build-profile release --build-features default --keep-work-dir
```

选择位于待测文件系统的 work-dir；本环境 `/tmp` 是 tmpfs，正式运行使用 `/workspace` 下独立工作目录，并将子进程 TMPDIR 指向该目录中的 scratch。报告自动记录实际文件系统和规范化策略；本报告同样可由上述相对目录复现工作流，但不同机器的数字不能视为同机 A/B。状态为 `COMPLETE_WITH_FAILURES` 时退出码为 1，应检查真实失败项，不自动提高预算后覆盖报告。

后续 `1f1f29a` 同时增加 session 计时与 guard 配置，并改变脚本 hash。其默认 content guard 也不同于本基线的双 guard。做严格前后比较，应冻结本次 `46e0fb5` 方法用于两侧，或者用同一新脚本、相同 `--read-transport file --snapshot-guard manifest` 配置重新成对测量。不能把方法变化隐藏成加速比。比较工具的调用形式为：

```sh
python3 -B benchmarks/compare_kernel_baseline.py \
  benchmarks/kernel-baseline/46e0fb5-linux.json data/kernel-after.json

python3 -B -m unittest discover -s tests/benchmarks -v
```

比较工具检查生成器、文件和查询/Shapes/rules 摘要、样本计划、预算、并发、缓存/scratch 策略、CPU/affinity 数、工具链与文件系统等关键记录。它允许引擎身份与构建提交变化；仅从环境比较中排除编译期 `ONTOKILN_BUILD_REVISION`，仍记录原始构建身份。真正运行参数如 Rayon/Tokio 线程数、pack 上限和 TMPDIR 策略仍严格比较。

任何失败、跳过、缺失、未通过 oracle 的请求，或任一侧不足三个成功测量样本，都会隐藏该阶段的改进百分比，同时保留状态与可用原始中位数。配置一致也不证明物理宿主、底层设备和外部负载完全相同；比较数字是描述性观察，不是显著性证明或自动发布门槛。

## 10. 回归验证

被测源码已经通过 `cargo fmt --all --check`、Clippy `-D warnings`、默认及 all-features workspace tests、32 组 API 回归、15 组 Full 回归，以及 14/14 scenario 比较。冻结消费者的 10 个旧请求仍在 14 个后端场景重放；release 二进制另跑六项新增契约回归。基准与比较工具的初版验证包含 33 个 Python 单测；合并 session 工具并补充扩展测量驱动及比较规则后，发布工具合计 52 项 Python 单测通过，覆盖独立答案、失败/跳过、watchdog、构建身份和可比较性规则。

源码提交 `46e0fb5` 的 [Consumer contracts CI](https://github.com/Codefor/ontokiln/actions/runs/43358335296) 已成功；报告提交的 CI 应以该提交自己的运行记录为准。历史 10M/100M pack 容量套件没有重跑。本报告的实测范围为 10K / 100K / 1M；额外的 1M / 10M 同内核扩展实验见独立规模报告。
