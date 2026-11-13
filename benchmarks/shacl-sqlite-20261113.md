# Full SHACL SQLite 邻域读取实测（2026-11-13）

本轮将严格解析的 Full 快照上的普通 SHACL Core 校验改为按需读取 SQLite 邻域，复用既有 Rudof 约束求值器。固定一个 focus 时，100K 背景图的通过用例由 **629.812 ms 降至 11.196 ms，前 / 后为 56.25×**；原生 minCount 违规用例由 **660.891 ms 降至 11.328 ms，前 / 后为 58.34×**。两项 API 峰值 RSS 均从约 **157 MiB 降至 18 MiB**。会话中的每次校验仍启动独立 worker，收益不依赖跨请求结果缓存。[focused 完整比较](shacl-sqlite-20261113/focused-comparison.md)

在同样的 512 MiB 请求预算下，1M 背景图的两个 Core 用例从预热即预算失败变为每项预热及 3 次正式请求全部完成，正式中位数分别为 **11.851 / 11.549 ms**。非局部路径与 SPARQL 用例仍会建立完整查询图，1M 前后均预算失败。失败请求没有完成同等工作，**不计算百万级加速倍数**。

全 targets 的改善幅度较小：100K 文件 API 的 targetSubjectsOf 全量校验由 **1,006.814 → 922.887 ms**，显式 targetClass 全量校验由 **1,117.728 → 861.153 ms**，RSS 从约 **166 → 39 MiB**。同时保留回退：10K 类目标校验 **+12.50%**、100K 非局部单 focus 校验 **+13.38%**、100K Full 物化 **+12.52%**。这些结果分别属于下文列明的会话与文件入口，不能跨入口互算收益。[kernel 完整比较](shacl-sqlite-20261113/kernel-comparison.md)

## 数据、版本与身份

| 测量组 | 前版原始记录 | 新版原始记录 | 自动比较 |
|---|---|---|---|
| 固定 focus，常驻会话，10K / 100K / 1M | [before-focused-session.json](shacl-sqlite-20261113/before-focused-session.json) | [after-focused-session.json](shacl-sqlite-20261113/after-focused-session.json) | [JSON](shacl-sqlite-20261113/focused-comparison.json) · [Markdown](shacl-sqlite-20261113/focused-comparison.md) |
| kernel 文件 API，10K / 100K | [before-kernel-file.json](shacl-sqlite-20261113/before-kernel-file.json) | [after-kernel-file.json](shacl-sqlite-20261113/after-kernel-file.json) | [JSON](shacl-sqlite-20261113/kernel-comparison.json) · [Markdown](shacl-sqlite-20261113/kernel-comparison.md) |

| 项目 | 前版 | 新版 |
|---|---|---|
| 被测源码提交 | [`5ad597f4b8c0f02fc46c58b0dda59ed4ef3a6528`](https://github.com/Codefor/ontokiln/commit/5ad597f4b8c0f02fc46c58b0dda59ed4ef3a6528) | [`f5b8c5f05a3de9313191e4a9ffeebab16df16648`](https://github.com/Codefor/ontokiln/commit/f5b8c5f05a3de9313191e4a9ffeebab16df16648) |
| 完整代码树 | `ab5fa79281eb0b6f4f38a8e96fb74acd4e77897f` | `d52cd84ebc896ac33f38942159fcae9e2ca89eeb` |
| 二进制 SHA-256 | `ba608797358398cc1ba6521d17077fa7137d995da2bcf74296f65471962b0fca` | `4aca58f157d080636673dece73179ccd32222947142014f1cdd7416d94801064` |
| 内核指纹 | `f5f1ee72a28194aa2ab8cb647210d25157cb7e5357e8aae5393c870d8c22201c` | `faf847360784cfd5c8fe6279219eeca3119624bae2f5217d0d7c872c833a1c39` |

[构建与验证记录](shacl-sqlite-20261113/build-receipt.json)保存构建命令、二进制大小、日志摘要、代码树对应关系及原始文件摘要。前版在独立的固定提交工作树中构建；原始 JSON 的 `environment.repository_head` / `repository_dirty` 描述的是运行 harness 的工作目录，并不代替二进制源码身份。被测版本以 `kernel_identity.source_revision` 和二进制摘要为准。

本报告及原始 JSON 通过后续报告提交入库，报告提交与上述被测代码提交分开。新版完整门禁在本地提交 `1a6a640cbdf1f44cd263b43f9cf0db6a78f2f967` 上执行；其 Git tree 与已推送 `f5b8c5f` 相同，随后以已推送提交重新构建被测 release 二进制。这里不把不同的提交号当成相同源码的唯一证明，代码树核对结果保存在构建记录中。

## 环境与计量边界

| 项目 | 本轮记录 |
|---|---|
| 系统与 CPU | Linux 6.18.57 x86_64，glibc 2.41；AMD EPYC 9V74；环境可见 9 个逻辑 CPU |
| 可见物理内存 | 10,451,427,328 字节，约 9.73 GiB |
| **实际数据文件系统** | **`/tmp` 的 tmpfs**；harness 未指定 `--work-dir`，输入、产物、快照和请求 scratch 位于该临时工作目录；不能将它描述成 `/workspace` overlay 的磁盘成绩 |
| 构建 | Rust 1.95.0，release，default features，`Cargo.lock` 固定；构建在计时外 |
| 并发 | 请求串行，Rayon 1 线程，Tokio 1 worker；本任务的构建与门禁不与正式计时并发 |
| Full 请求预算 | RSS 512 MiB、scratch 4 GiB、60 s；最多 100 轮物化、100 万推导事实、1,000 万中间候选行；验证深度 64 |
| 快照前置条件 | 同时绑定内容摘要与原始 manifest SHA-256；前后各自重新编译匹配其内核身份的产物 |
| 采样 | 每项读取预热 1 次、正式 3 次；每项编译 / 物化独立执行 3 次；每份 compiled 产物的会话挂载只观测 1 次 |
| 缓存状态 | 未清空 OS 缓存；生成、摘要核对和编译均可能预热文件页；每次读取仍新建 worker |
| 统计 | 成功正式样本的中位数与范围；变化 = `(after / before - 1) × 100%`，负数表示更低；不估计 P95/P99 或统计显著性 |

会话读取从写入完整 JSONL 请求到收到完整响应行计时，排除一次性会话启动 / 挂载。文件 API 从创建进程到进程退出及读取结果计时，包含每次快照准备。两者都排除 Python JSON 解析、oracle 核验和计时后的产物检查；API 的执行时间是另一项指标，不能再与端到端时间相加。编译和物化即使出现在 focused 会话报告中，也使用独立文件 API 进程。[计量定义](../tests/benchmarks/README.md)

API `peak_rss_bytes` 是监督进程与 worker 的预算计量，含采样和共享页影响，不是 PSS；它与 `process_wait4_peak_rss_bytes`、监督进程单列 RSS 不能简单相加。scratch 与产物字节表示占用规模；tmpfs 成绩不等同于物理磁盘 I/O 性能。可见 cgroup 限额字段为 null，不能据此认定无资源限制。同机元数据也不能证明云宿主机负载或 OS 缓存状态完全相同。

两组自动比较均为 `comparable=true`、`issues=[]`、`methodology_changed=false`。四份报告使用同一 harness 字节，SHA-256 为 `102ae92f3dad4cda61507a855a448535f7054ede10857914fb93cdd18b179139`。各组前后输入、规则、Shapes、预算、transport 和 stage metadata 均一致；`focused-shacl` 的额外 Shapes 只属于该套件，未改变原 kernel 套件的输入摘要。

## 实现改动与保留的执行范围

[固定提交的 SQLite SHACL 适配器](https://github.com/Codefor/ontokiln/blob/f5b8c5f05a3de9313191e4a9ffeebab16df16648/src/shacl_sqlite.rs)通过已有 SPO / POS / OSP 索引读取匹配邻域，Rudof 继续执行原有 Core 约束。显式 focus 跳过目标展开；未指定 focus 时，显式 class targets 继续沿逆向子类和类型关系选择目标。每次验证仍保留完整目标 coverage，未增加隐式 class target 或按业务 IRI 分派的分支。

适配器中的缓存都属于本次 worker：

- 词项 ID 与解码后词项分别最多 4,096 项、估算 4 MiB；SQLite page cache 也属于该 worker。
- 最多保留**最近一次完整的 subject / predicate 邻域**，最多 4,096 个对象、估算 1 MiB，用于组合求值器与 Rudof 复用同一次读取。
- 不完整、超出缓存上限或被中途拒绝的邻域不进入缓存；超限不截断结果或跳过约束。worker 退出后这些缓存全部释放，会话不保留上一请求的验证答案。

读取视图继续是所有存储图中显式与推导事实的去重 SPO union。复杂路径、实际执行的 SHACL-SPARQL 约束 / target / 自定义查询仍按需建立完整 union 查询图；`shapes_from_dataset` 仍读取完整 Shapes 图；历史宽松解析或解析策略不明的产物保留原有全图词项校验路线。focus 选择不缩小这些查询可见的数据。SQLite 读取或解码错误会保持在适配器中，原生约束不能通过吞掉错误把失败变成 `conforms=true`；中断和资源错误保留预算错误类型。

因此，全 targets 的目标集合、coverage、约束访问和结果缓存仍随实际工作增长。此次测量支持普通 Core 的单 focus 避免预先全图装载，并不建立全量 SHACL 的恒定内存保证。[Full 合同](../docs/full.md#shacl) · [会话边界](../docs/session-v1.md#performance-boundaries) · [固定提交的校验入口](https://github.com/Codefor/ontokiln/blob/f5b8c5f05a3de9313191e4a9ffeebab16df16648/src/shacl.rs)

## 固定 focus：10K / 100K / 1M 背景图

### 工作负载与独立预期

三个规模分别有 10,000 / 100,000 / 1,000,000 条去重显式事实，对应 999 / 9,999 / 99,999 条合成记录。所有规模固定选择 `order/000000008`，每个请求只选择一个 Shape、一个 focus；局部约束工作保持不变，背景数据量增长。此套件只 Full compile、可选 session mount 和验证，不执行物化或独立查询阶段。

| 工作负载 | 必须核对的结果 |
|---|---|
| `focus_core_pass` | customer / amount 的原生 Core 约束通过；一个 focus、完整 coverage、零违规 |
| `focus_core_fail` | amount 只有一个值而 minCount 为 2；必须出现正确 focus / path / named shape 的唯一原生违规 |
| `focus_nonlocal_fail` | 两跳 customer → region 路径必须可见；SPARQL 必须找到无关 `order/000000000` 的背景记录，并返回正确来源组件的唯一违规 |

前后所有成功样本逐项核对顶层状态、`checked_shapes`、完整 coverage、target_count 和 violations 的字段集合。诊断 message 与 build identity 排除在语义摘要之外，避免源码 revision 变化导致伪不一致；结构化答案仍与独立预期精确相等。同一用例的成功语义摘要在规模与 binary 之间保持一致。非局部负控制也会拒绝“背景约束漏算、只产生其他违规”的伪结果。[生成器与 oracle](kernel_baseline.py) · [oracle 回归](../tests/benchmarks/test_kernel_baseline.py)

### 10K 与 100K 会话请求

以下每行前后均预热成功、正式样本各 3 次。时间单位为 ms，RSS 为 MiB；这些数字不包含本节之后单列的挂载。

| 显式事实 | 用例 | 前版 ms | 新版 ms | 变化 | 前 / 后 | API RSS 前 → 后 MiB |
|---:|---|---:|---:|---:|---:|---:|
| 10,000 | Core 通过 | 82.867 | 11.456 | -86.18% | 7.23× | 30.887 → 17.785 |
| 10,000 | Core 违规 | 42.227 | 11.738 | -72.20% | 3.60× | 31.320 → 17.871 |
| 10,000 | 非局部违规 | 62.183 | 52.523 | -15.53% | 1.18× | 32.027 → 32.961 |
| 100,000 | Core 通过 | 629.812 | 11.196 | -98.22% | 56.25× | 157.438 → 17.984 |
| 100,000 | Core 违规 | 660.891 | 11.328 | -98.29% | 58.34× | 157.398 → 17.770 |
| 100,000 | 非局部违规 | 679.973 | 770.966 | **+13.38%** | 0.88× | 151.652 → 154.660 |

100K Core 通过的正式范围为 **620.180–689.795 → 11.045–11.205 ms**；Core 违规为 **598.437–751.057 → 11.262–11.497 ms**。RSS 中位数分别降低 **88.58% / 88.71%**。10K Core 通过的前版范围较宽，为 72.065–269.893 ms，不能用其单个慢样本代替中位数扩大收益。

100K 非局部用例明确回退：正式范围 **669.696–733.592 → 639.567–893.741 ms**，RSS 中位数增加约 3.01 MiB。两版仍执行完整查询图；三个样本的区间重叠，现有计量不独立拆分图装载、求值和调度，不能将全部差值确定归因于某一内部步骤。[逐样本与自动比较](shacl-sqlite-20261113/focused-comparison.json)

### 1M：完成能力与预算失败分别保留

| 用例 | 前版预热 / 正式 | 新版预热 / 正式 | 新版正式中位数 | 新版 API RSS 中位数 |
|---|---|---|---:|---:|
| Core 通过 | 1 次 `BUDGET_EXHAUSTED`；3 次 `SKIPPED` | 1 次 `COMPLETE`；3 次 `COMPLETE` | 11.851 ms | 18.035 MiB |
| Core 违规 | 1 次 `BUDGET_EXHAUSTED`；3 次 `SKIPPED` | 1 次 `COMPLETE`；3 次 `COMPLETE` | 11.549 ms | 17.777 MiB |
| 非局部违规 | 1 次 `BUDGET_EXHAUSTED`；3 次 `SKIPPED` | 1 次 `BUDGET_EXHAUSTED`；3 次 `SKIPPED` | — | — |

新版 Core 通过与违规的正式范围分别为 **11.705–11.992 ms**、**11.479–15.340 ms**。背景图从 10K 增至 1M 时，这两项的新版中位数保持约 11–12 ms、RSS 约 18 MiB；这是固定 focus 和固定局部邻域在本机该数据分布下的观测，不是任意 Shape / 高度数 focus 的复杂度上限。

前版三个实际失败请求的 API RSS 峰值约 **515.56 / 521.74 / 515.73 MiB**；新版非局部失败约 **515.11 MiB**，都超过 512 MiB 预算。监督器采样并终止 worker，故记录值可略高于阈值。错误响应保留 `RESOURCE_EXHAUSTED` / `BUDGET_EXHAUSTED`；约 4 s 的失败耗时不参与成功中位数。harness 在首个失败后停止该用例剩余重复并显式记为 SKIPPED，未调高预算重跑来替换这组结果。[前版记录](shacl-sqlite-20261113/before-focused-session.json) · [新版记录](shacl-sqlite-20261113/after-focused-session.json)

## 文件 API：全 targets 与既有 SHACL 用例

本组每次请求包含进程创建、核验 / 快照准备、校验与读取响应。`all_orders` 使用 targetSubjectsOf，`class_orders` 使用显式 targetClass；10K / 100K 分别精确核对 **999 / 9,999 个目标**，没有把少算目标计为优化。其余三个用例是原有单 target 负控制和 compiled / reasoned 事实可见性检查；它们未改成 focused suite 的调用方式。

| 显式事实 | 工作负载 | 前版 ms | 新版 ms | 变化 | API RSS 前 → 后 MiB |
|---:|---|---:|---:|---:|---:|
| 10,000 | 全 targetSubjectsOf | 136.069 | 95.132 | -30.09% | 32.020 → 20.215 |
| 10,000 | 全显式 targetClass | 85.189 | 95.839 | **+12.50%** | 32.027 → 20.258 |
| 10,000 | 原生 Core minCount 负控制 | 53.902 | 13.092 | -75.71% | 30.195 → 17.629 |
| 10,000 | compiled 事实探针 | 33.875 | 12.907 | -61.90% | 20.512 → 17.379 |
| 10,000 | reasoned 事实探针 | 54.168 | 12.665 | -76.62% | 23.453 → 17.484 |
| 100,000 | 全 targetSubjectsOf | 1,006.814 | 922.887 | -8.34% | 166.438 → 38.902 |
| 100,000 | 全显式 targetClass | 1,117.728 | 861.153 | -22.96% | 166.480 → 39.461 |
| 100,000 | 原生 Core minCount 负控制 | 695.326 | 43.704 | -93.71% | 156.344 → 17.336 |
| 100,000 | compiled 事实探针 | 288.170 | 44.411 | -84.59% | 46.039 → 17.312 |
| 100,000 | reasoned 事实探针 | 583.645 | 53.918 | -90.76% | 69.508 → 17.316 |

100K 两项全量校验的 RSS 中位数分别降低 **76.63% / 76.30%**。延迟范围为：全 targetSubjectsOf **974.088–1,027.163 → 875.646–945.170 ms**；全显式 targetClass **1,056.054–1,167.521 → 854.222–939.393 ms**。10K 类目标回退的范围为 **85.022–85.325 → 95.553–105.397 ms**，本轮保留其完整记录。逐 target 的索引读取和约束求值仍然有成本，不能将单 focus 的约 56× 收益外推给全 targets。[kernel 比较](shacl-sqlite-20261113/kernel-comparison.json)

## 编译、物化、其他查询与空间代价

### 独立构建

kernel 组的每项构建都从独立请求生成完整产物，前后各 3 次：

| 显式事实 | 阶段 | 前版 ms | 新版 ms | 变化 |
|---:|---|---:|---:|---:|
| 10,000 | pack 编译 | 26.518 | 20.668 | -22.06% |
| 10,000 | Full 编译 | 97.634 | 97.749 | +0.12% |
| 10,000 | Full 物化 | 116.807 | 117.593 | +0.67% |
| 100,000 | pack 编译 | 199.270 | 210.852 | +5.81% |
| 100,000 | Full 编译 | 845.704 | 826.803 | -2.23% |
| 100,000 | Full 物化 | 1,053.643 | 1,185.529 | **+12.52%** |

100K Full 物化正式范围为 **981.752–1,109.496 → 1,163.556–1,260.891 ms**，API RSS 中位数为 **53.625 → 54.730 MiB**。两版都得到 **86,240 条推导事实、186,240 条合计事实**；10K 都得到 8,615 条推导事实。规则 profile 仍为 `rdfs-supported-v1` 加 3 条显式正向规则。本轮未修改规则物化算法，以上是前后实测差异，不能据此确定 SHACL 邻域实现造成了全部物化回退。[kernel 原始新版](shacl-sqlite-20261113/after-kernel-file.json)

focused 组单独运行的 Full 编译也完整保留，不挑选其中较快一组替换 kernel 构建数据：

| 显式事实 | 前版 ms | 新版 ms | 变化 | API RSS 前 → 后 MiB |
|---:|---:|---:|---:|---:|
| 10,000 | 86.779 | 97.995 | +12.93% | 18.742 → 19.199 |
| 100,000 | 812.876 | 752.383 | -7.44% | 45.746 → 46.719 |
| 1,000,000 | 8,937.535 | 8,423.061 | -5.76% | 166.430 → 167.539 |

1M 编译的 API scratch 峰值中位数为 **368.821 → 373.712 MiB**；产物大小没有随之增加，scratch 还包含运行中的中间文件与采样影响。前后编译均在本轮相同预算内完成。[focused 构建记录](shacl-sqlite-20261113/focused-comparison.json)

### 非 SHACL 查询回归

下表列出 kernel 组全部 12 类查询的端到端中位数，前后每项 3 次正式样本；这是文件 API 口径。本轮不以这些波动声称新增了 SPARQL 优化。

| 工作负载 | 10K 前 → 后 ms | 10K 变化 | 100K 前 → 后 ms | 100K 变化 |
|---|---:|---:|---:|---:|
| compiled 点查 | 13.276 → 13.350 | +0.56% | 43.405 → 43.476 | +0.16% |
| compiled 全事实计数 | 24.293 → 23.199 | -4.51% | 104.564 → 94.764 | -9.37% |
| compiled 有界路径 | 13.315 → 12.884 | -3.23% | 43.873 → 44.272 | +0.91% |
| compiled customer-region 连接 | 23.842 → 23.678 | -0.69% | 94.745 → 95.339 | +0.63% |
| compiled 区域聚合 | 66.780 → 53.621 | -19.71% | 500.251 → 541.086 | **+8.16%** |
| reasoned 推导谓词计数 | 23.439 → 23.293 | -0.62% | 114.651 → 114.745 | +0.08% |
| reasoned closure 连接 | 13.330 → 13.506 | +1.32% | 54.098 → 55.169 | +1.98% |
| pack 点查 | 13.074 → 12.923 | -1.16% | 23.202 → 24.050 | +3.65% |
| pack 全事实计数 | 12.779 → 12.911 | +1.03% | 33.542 → 33.164 | -1.13% |
| pack 有界路径 | 12.943 → 13.300 | +2.75% | 23.459 → 23.288 | -0.73% |
| pack customer-region 连接 | 13.075 → 13.409 | +2.56% | 23.915 → 23.846 | -0.29% |
| pack 区域聚合 | 12.920 → 13.266 | +2.67% | 43.811 → 34.721 | -20.75% |

100K compiled 区域聚合的范围为 **491.129–523.432 → 520.047–560.969 ms**，保留为回退。其他样本亦有波动，例如新版 reasoned 推导谓词计数为 104.524–185.356 ms；查询核心时间未独立暴露，不能把端到端变化完全归因于内部求值。非 SHACL 请求的 API RSS 多增加约 1–2 MiB，原始值、scratch 和其他指标在[完整比较](shacl-sqlite-20261113/kernel-comparison.md)中保留。

### 产物与工作空间

本轮没有新增数据库索引。前后对应产物的文件长度一致，下表同时核对 `facts.sqlite` 与完整产物目录的逻辑字节。两组全部 21 对 Full 构建样本的 manifest SHA-256 与 content digest 也分别相同，具体结果保留在原始记录；这些比较不替代既有格式、身份和摘要准入检查。

| 产物 | facts.sqlite 字节，前 = 后 | 完整目录逻辑字节，前 = 后 |
|---|---:|---:|
| 10K compiled | 1,409,024 | 2,615,072 |
| 100K compiled | 13,369,344 | 25,397,897 |
| 1M compiled，focused 组 | 139,964,416 | 260,307,974 |
| 10K reasoned | 1,867,776 | 3,073,976 |
| 100K reasoned | 18,309,120 | 30,337,824 |

pack 文件也保持 10K 为 394,534 字节、100K 为 3,886,533 字节。release 二进制由 **21,754,776 → 22,123,576 字节**，增加 368,800 字节、约 **1.70%**。[产物记录](shacl-sqlite-20261113/after-kernel-file.json) · [构建记录](shacl-sqlite-20261113/build-receipt.json)

100K 会话 Core 通过 / 违规的请求 scratch 中位数由 **12.108 / 11.581 MiB 降至约 3.5 / 3.7 KiB**，但挂载仍另持有 **13,369,344 字节**的私有数据库，不能把请求级小 scratch 描述成零存储占用。文件 API 的全 targetSubjectsOf / targetClass scratch 中位数分别为 **23.877 → 13.192 MiB**、**24.408 → 13.192 MiB**，其中包含每次请求自己的快照准备。不同入口的 scratch 边界同样不能混用。

## 一次性挂载与首次请求

本轮前版 / 新版成功的会话验证共 **24 / 32 次**，包含预热；每次成功响应都核对 `snapshot_reused=true`、`snapshot_bytes_copied=0`。这表示读取复用了已核验的数据库快照，词项、邻域、查询计划和验证结果没有跨 worker 保留。

| 显式事实 | 进程创建至 READY，前 → 后 ms | READY 内部挂载，前 → 后 ms | 每次挂载复制字节，前 = 后 |
|---:|---:|---:|---:|
| 10,000 | 5.432 → 10.634 | 3.549 → 8.003 | 1,409,024 |
| 100,000 | 41.818 → 34.365 | 39.032 → 32.633 | 13,369,344 |
| 1,000,000 | 349.021 → 333.583 | 344.981 → 330.943 | 139,964,416 |

每个数值只有 **1 次观测**，比较器不输出挂载变化百分比。它们是 tmpfs、未清空 OS 缓存环境中的新实例启动观测，不是冷盘分布、稳定上限或生产 SLO。一次新实例仍需核验与私有快照复制；首个请求还要启动 worker、打开 SQLite 和准备本次 Shapes / 求值状态。多请求可以摊销挂载，不能消除挂载本身。[会话原始新版](shacl-sqlite-20261113/after-focused-session.json) · [生命周期合同](../docs/session-v1.md#cold-starts-and-application-rollout)

## 正确性、失败记录与发布

四份正式报告共 **404 条计划记录**，含构建、挂载和预热：**388 条 COMPLETE、4 条实际 BUDGET_EXHAUSTED、12 条 SKIPPED**。其中 kernel 两份各 154 条 COMPLETE；focused 前版为 36 COMPLETE / 3 预算失败 / 9 跳过，新版为 44 COMPLETE / 1 预算失败 / 3 跳过。所有成功请求均通过对应独立 oracle；预算失败和跳过都保留在 JSON 中。

比较器只让 `COMPLETE` 且 `oracle.checked=true` 的正式样本进入中位数；预热失败、正式失败、跳过、缺失或未验证样本都会禁止该 stage 的百分比。`comparable=true` 表示记录配置可比，不表示所有 workload 已完成。跨 binary 的 source revision 留作来源身份，不加入语义报告摘要。[比较器](compare_kernel_baseline.py)

在与被测新版相同的代码树上，本地已完成：

- `cargo fmt --all --check`；workspace / all-targets Clippy，警告视为错误。
- 默认及 all-features workspace 测试；相关覆盖包括 34 项 native 测试（含 15 项 SQLite store、7 项差分验证）、35 组 API、8 组真实进程 session、9 组 SHACL、15 组 Full 回归。它们是对应测试集合的计数，不重复相加成另一项独立门禁。
- `scenario_lab --compare`：14/14。
- harness 与比较器 Python 单元测试：61/61，包含伪结果、失败 / 跳过、计时边界与可比性检查。
- release smoke 与本报告四份正式测量，完成状态如上。

回归覆盖图和显式 / 推导事实的 union 去重、原有 class / target 语义、字面量及空白节点、闭合与组合约束、非局部查询、读取错误、资源中断、缓存上限 / 不完整读取、源文件变化、预算、取消及 worker 回收。构建和本地门禁的日志摘要见[build-receipt.json](shacl-sqlite-20261113/build-receipt.json)。代码提交 `f5b8c5f` 的[远端 CI](https://github.com/Codefor/ontokiln/actions/runs/43367612991)已完成并通过；这是被测代码提交的状态，报告后续提交的状态另行查看。

**API v1 的请求字段、响应字段和既有校验语义保持不变。** 调用方升级实际运行的二进制即可使用新执行路径；文件 API 也使用相同适配器，会话入口继续提供跨请求快照复用。现有兼容产物继续按原格式、身份和摘要规则使用，本轮不新增一律重建产物的要求。原生 `kernel-manifest-v2` 的既有规则要求 manifest 的 kernel 与运行内核身份完全一致；只有这类身份不匹配时，才需要重建或切换到匹配的 compiled / reasoned 产物，并使用相应摘要前置条件。不能据此承诺任意旧产物跨 kernel 无条件可用。本报告为独立比较构建性能，每个版本都用自己的二进制重新构建产物。[固定提交的产物准入](https://github.com/Codefor/ontokiln/blob/f5b8c5f05a3de9313191e4a9ffeebab16df16648/src/full_artifact.rs) · [API v1 合同](../docs/api-v1.md)

替换实例时，先挂载新产物并执行代表性读请求，再由应用切换接收新请求的实例，旧实例等待在途 worker 结束。业务路由、发布原子性和实例切换仍由应用适配层负责。

## 复现与范围

分别从上述固定代码提交构建 release 二进制，保留在不同路径。构建命令为 `cargo build --release --offline --locked --bin ontokiln`；`--offline` 要求依赖已经可用。两次测量统一使用 `f5b8c5f` 中的 [kernel_baseline.py](https://github.com/Codefor/ontokiln/blob/f5b8c5f05a3de9313191e4a9ffeebab16df16648/benchmarks/kernel_baseline.py)，不能分别使用两个提交中不同版本的 harness。每次只替换 binary 和输出路径，不在计时期间并发构建或运行门禁。

固定 focus 会话组：

```sh
TMPDIR=/tmp python3 -B benchmarks/kernel_baseline.py \
  --binary /absolute/path/to/before-or-after-ontokiln \
  --output /absolute/path/to/focused-session.json \
  --suite focused-shacl --read-transport session \
  --sizes 10000 100000 1000000 \
  --build-samples 3 --query-samples 9 --validation-samples 3 \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 --threads 1 \
  --build-profile release --build-features default --snapshot-guard manifest
```

该套件不执行独立查询，`--query-samples 9` 只保留本次报告中的参数值。kernel 文件组：

```sh
TMPDIR=/tmp python3 -B benchmarks/kernel_baseline.py \
  --binary /absolute/path/to/before-or-after-ontokiln \
  --output /absolute/path/to/kernel-file.json \
  --suite kernel --read-transport file --sizes 10000 100000 \
  --build-samples 3 --query-samples 3 --validation-samples 3 \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 --threads 1 \
  --build-profile release --build-features default --snapshot-guard manifest
```

分别比较同一组的前后 JSON，例如：

```sh
python3 -B benchmarks/compare_kernel_baseline.py \
  benchmarks/shacl-sqlite-20261113/before-focused-session.json \
  benchmarks/shacl-sqlite-20261113/after-focused-session.json \
  --format markdown --output /absolute/path/to/focused-comparison.md
```

kernel 组使用相应的 `before-kernel-file.json` / `after-kernel-file.json`。复现前核对 `/tmp` 的实际文件系统；在其他介质上运行时应保留其环境元数据，不与本次 tmpfs 成绩直接等同。

本轮没有测 1M 全 targets SHACL、生产数据、macOS、HTTP 吞吐或多租户并发。合成输入采用有限词项长度、固定局部度数和浅层关系，真实高扇出邻域、复杂 Shapes 或全局查询需要另测。百万级 Core 单 focus 的成功不扩展为完整 SHACL、Full 亿级容量或物理磁盘性能结论；此前 pack 亿级与编译 / 物化规模报告各自保持其原有计量范围。
