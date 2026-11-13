# SHACL 绑定驱动索引连接实测（2026-11-13）

本轮让符合保守条件的左侧新绑定逐行进入右侧三元组索引读取。相同 512 MiB / 60 s 预算下，1M 背景的 nonlocal 从 **143.354 降至 11.498 ms（12.47 倍）**，独立 path 从 **328.404 降至 11.645 ms（28.20 倍）**，三目标 path 从 **857.492 降至 11.434 ms（74.99 倍）**。这三项 API RSS 从约 22.7 降至 18.8–19.0 MiB。

增长左侧的 fanout 在 100K 从 **1028.773 降至 73.209 ms（14.05 倍）**；1M 前版预热耗尽 60 s 预算，新版预热和三次测量均完成，中位数 **558.383 ms / 36.406 MiB**，不对失败前版计算倍数。真正全图聚合在 1M 仍约 **1.52 s**，成本不会因为固定 focus 而消失。

被测连接优化未启用完整逻辑优化器，不改变 API v1 字段或查询多重集语义。收益不覆盖所有阶段：100K Core pass 本次中位数反而 **+51.35%**，多项 kernel 查询、全目标校验和物化也变慢，完整范围与限制保留于第 5–6 节。最终集成还包含并行快照读取兼容修复，其准入边界单列于第 10 节。额外 pack 100M 两模板已完成，EXISTS 的小幅回退同样保留；这些结果不外推到所有查询或 Full 亿级容量。

## 1. 固定代码、二进制与测量来源

| 身份 | 正式 before | 正式 after |
|---|---|---|
| 二进制源码 | [`b8ecf8046a286345926077318a78dde7541fb98b`](https://github.com/Codefor/ontokiln/commit/b8ecf8046a286345926077318a78dde7541fb98b) | [`ed7b90d8feb2c00b87a4dc4981a2c36d67325355`](https://github.com/Codefor/ontokiln/commit/ed7b90d8feb2c00b87a4dc4981a2c36d67325355) |
| 完整 Git tree | `3347b0b9f9e0fb75286df19592b5254bdf6a1b11` | `acf9698ea116f9e65be3a60de627f47e5d968b67` |
| kernel SHA-256 | `ccb28441555497f8cd541412cce5a02eca371d4853b551c5674de2208d9c8b78` | `209f68f0aa4a7f7669d4b0ccaab27557ee8ee49cfb9bcaf2a41a3b00e7586f3c` |
| binary SHA-256 | `83680cfeca52cdd201ba02f9c40fa5bca19ad18a1a9443942054df83999d78c1` | `c55432609a454dfb916b9d6e49183be93421ecc05098ee7b77c5e2716143195b` |
| binary 字节 | 22,995,720 | 23,050,400 |

before 复用[上一轮构建记录](shacl-query-sqlite-20261113/build-receipt.json)中的最终 b8 binary，保存文件的实际 SHA/大小已重新核对。上轮的端到端计量没有拼入本轮：前后都使用新 harness 完整重测。原始报告中的 repository HEAD 表示运行 harness 的工作区，不替代 binary 的内嵌来源身份。

本轮保留并行合入的[业务仓库基准发现工具](https://github.com/Codefor/ontokiln/commit/1610e8a7898b476109c7ddb2b5b64f543065f0dd)。它不属于这里的引擎性能改动或计量工作负载。`1610… → ed7…` 的本轮差异是物理连接实现、回归、harness 扩展及说明；源码提交和后续报告提交分别记录。

测量完成后，远端加入了独立的 [Full 快照读取兼容修复 `796ac314ae7f6b8d6fb8f8e3cb0b4e06d90051c2`](https://github.com/Codefor/ontokiln/commit/796ac314ae7f6b8d6fb8f8e3cb0b4e06d90051c2)。已创建无代码冲突的双父集成提交 [`6d7f1e3aa9b8654b91d7907a6eb343b714aa7add`](https://github.com/Codefor/ontokiln/commit/6d7f1e3aa9b8654b91d7907a6eb343b714aa7add)，tree 为 **`14f9a51fb914ab3486e5799d674cef119efa3549`**，parents 为 ed7 与 796a，保留完整测量历史。该修复修改 API/产物/挂载代码，改变了 kernel fingerprint，但没有修改 spareval、SQLite 查询实现或正式 harness；**500 条正式记录以及额外 pack 前后比较始终归属于 b8/ed7，不标作 merge 版本的实测数据**。6d7 的独立完整门禁及 100-fact smoke 已通过，详见第 9 节。

| 最终集成 6d7 的验证身份 | 值；仅用于集成验证，不对应上表正式计量 |
|---|---|
| kernel SHA-256 | `51b8f01f78ab4406b3fa4fbbeee69d60737515aaffbbbc92f38e5376e37b3bb7` |
| binary SHA-256 | `3b70b345b8c9f34247c89e727126d872b00881654dbd423875be965ad9eb8c04` |
| release binary / 构建耗时 | 23,057,824 字节 / 27.89155 s；release、default features |

| 正式组 | 前版附件 | 新版附件 | 比较器 |
|---|---|---|---|
| focused，session，10K / 100K / 1M | [before-focused-session.json](shacl-bind-joins-20261113/before-focused-session.json)，已完成 | [after-focused-session.json](shacl-bind-joins-20261113/after-focused-session.json)，已完成 | [focused-comparison.json](shacl-bind-joins-20261113/focused-comparison.json)，可比、0 issues |
| kernel，file，10K / 100K | [before-kernel-file.json](shacl-bind-joins-20261113/before-kernel-file.json)，已完成 | [after-kernel-file.json](shacl-bind-joins-20261113/after-kernel-file.json)，已完成 | [kernel-comparison.json](shacl-bind-joins-20261113/kernel-comparison.json)，可比、0 issues |

正式四份共 **500 条计划记录：focused 前后各 96、kernel 前后各 154；496 COMPLETE、1 次实际预算失败预热、3 SKIPPED**。before 共 246 COMPLETE，新版 **250 条全部 COMPLETE**。COMPLETE 包含构建、mount 和预热，不等于 496 次正式查询；所有成功记录 `oracle.checked=true`。

独立机器可读审计见 [formal-audit.json](shacl-bind-joins-20261113/formal-audit.json)，状态 **PASS、issues=[]**，355,638 字节，SHA `980633c433e5e93d2fe46bb09f06a6ecffef392bd3465795af4ef10cff58bd57`。它核对四份冻结 raw、全部逐项语义/方法/输入对照、21 对 Full 与 6 对 pack 产物、80/84 次成功 session 读取；范围不包括随后集成版本或额外 pack 大规模运行。

## 2. 改动范围与保守准入

实现位于 [`vendor/spareval/src/eval.rs`](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/vendor/spareval/src/eval.rs)，边界见 [vendor 说明](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/vendor/spareval/ONTOKILN.md)和 [Full 合同](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/docs/full.md)。SHACL 的 [`PreparedQueries`](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/src/rdf_graph.rs)仍调用 `without_optimizations()`，避免在预绑定应用前错误折叠 FILTER / BOUND。

| 条件 / 情况 | 执行方式与理由 |
|---|---|
| 既有 optimizer 已提供非空 join keys | 保留原 IndexBindJoin / Always 路径和执行循环 |
| keys 为空，右侧为单个 QuadPattern，左侧保证绑定至少一个共享变量 | 成为新增候选；再看运行时原始输入是否已绑定全部这些变量 |
| 候选中至少有一个共享变量由左侧新产生 | 左侧每条成功解先与原始输入 combine；兼容时传给右 quad，逐行索引 probe |
| 所有相关共享变量已被外层输入绑定 | 继续原单次右扫描 / CartesianProductJoin，避免反复读取同一范围 |
| 复杂右子树或没有可证明的新共享绑定 | 保留原算法，不把任意子查询改成相关子查询 |

“保证绑定”的分析尊重 Project 列；UNION 取各分支交集；OPTIONAL / MINUS 只承认左侧保证；VALUES 某列存在 UNDEF 时不把它算作保证绑定。Group / Service / Lateral 不新增保证；Extend 只继承内部保证，不因 BIND 表达式看似确定就推断新保证。宁可少启用优化，也不把可能未绑定的变量当作可安全关联的键。

新 probe 在调用右侧前执行 `left_row.combine_with(original_input)`。左行与原外层绑定冲突时淘汰，左行遗漏的原绑定继续保留；这一点覆盖 FILTER EXISTS 内 BIND 重写同名变量的兼容性。对每条左行 flat_map 右结果，保留重复解和 RHS 多匹配，不改成集合，也不把一般值比较替换成 sameTerm。

`AdaptiveIndexBindJoin` 的 adaptive 只指外层绑定状态，不是按基数、成本或 fanout 自动切换。大左侧仍可能发出很多次索引 probe；排序、聚合、去重、路径和连接中间状态仍可能增长。不能据此宣称所有连接都加速、固定耗时或恒定内存。

错误继续沿左/右迭代器传播，SHACL 既有 sticky 存储错误检查保持。没有 ORDER BY 时仍按 API v1 的结果多重集解释，不承诺内部读取顺序或错误出现的相同时刻。完整图 union、严格 RDF 解析条件、只读数据和原有显式 targets 语义不变；kernel 身份准入的后续集成变更见第 10 节。

## 3. 方法、预算与缓存边界

| 项目 | 四份正式 raw 已核对的相同口径 |
|---|---|
| 机器 | Linux 6.18.57 x86_64、glibc 2.41；AMD EPYC 9V74；9 个可见逻辑 CPU；约 9.73 GiB RAM |
| 实际临时文件系统 | `/tmp` tmpfs；不能描述成冷磁盘或 overlay 磁盘吞吐 |
| 工具链 | release、default features、锁定依赖；Rust 1.95.0 / LLVM 22.1.2、SQLite 3.54.1 |
| 正式 Full 预算 | 串行，Rayon/Tokio 各 1；512 MiB RSS、4 GiB scratch、60 s，未放松预算 |
| 样本 | 每项读 1 次预热 + 3 次测量；编译/物化独立 3 次；每个 mount 1 次诊断观测 |
| harness | `benchmarks/kernel_baseline.py` SHA `3b6efa9fce11e9b5a32e2a28802654b305fc708fa42f07ed6997a58e2b452bb2` |
| 输入规模 | 10K / 100K / 1M 显式事实，对应 999 / 9,999 / 99,999 条记录；默认 kernel 输入未改 |

session 从写 JSONL 请求到读响应计时，排除一次性 mount；file 包含进程与单次快照准备。kernel 的 pack compile 为独立构建 CLI，不由 Full API 的 RSS/scratch 预算强制，仅受外层超时监控。分别同 transport 比较，不把 session/file 或不同轮次的成绩互除。focused 编译仍为独立 file API。两组比较器均 **`comparable=true`、`methodology_changed=false`、`issues=[]`**；不足成功样本的 fanout 1M 和单观测 mount 不输出百分比。正式计时期间没有并行构建、测试或大型分析。

会话只复用已验证快照；每请求仍有独立 worker。词项、SQLite page cache、最近完整邻域和本次验证计划/结果不跨 worker 保留；本轮不增加常驻查询结果缓存。OS 缓存不清空，生成/摘要/编译可能预热文件页。

耗时与 API RSS/scratch 报告中位数及范围；预热、失败和 SKIPPED 不进入成功统计。RSS 不是 PSS，不与父进程/监督进程峰值相加；tmpfs 字节不等于物理磁盘 I/O。三次正式样本不足以报告 P95/P99、统计显著性或生产 SLO。

## 4. 七个 focused 病例与独立 oracle

六个病例显式 focus 为 order `000000008`；多目标病例不传 `focus_node`，检查 Shape 的真实 target 集合。前五项延续上轮内容但重新测量，新增两项只扩展 focused Shapes。

| 病例 | 独立预期与覆盖目的 |
|---|---|
| `focus_core_pass` | Core customer/amount 通过，零违规；检查普通邻域路径没有退化 |
| `focus_core_fail` | amount 的 minCount 2，唯一正确原生违规 |
| `focus_nonlocal_fail` | 两跳 path 成功；无关 order 0 的语言标签产生唯一 SPARQL 违规，证明 focus 外数据可见 |
| `focus_path_fail` | 两跳 hasValue 正控制通过、负控制唯一正确违规；隔离复杂路径执行 |
| `focus_full_scan_fail` | 全 union COUNT 与每行 subject 字符长度 SUM 同时命中独立预期，输出唯一违规；真正全扫描对照 |
| `multi_target_path_fail` | targets 为去重后的 orders 7、8、最后一个；正式规模 3 targets / 5 violations，100-fact smoke 为 2 / 3 |
| `focus_fanout_join_fail` | region 0 全部 customer → order → amount，附重复 VALUES，精确 COUNT/SUM 命中后产生唯一违规 |

fanout 的查询核心为以下内容。它要求处理增长的左输入和 bag 重数，而不是只做一个 focus 的点查：

```sparql
PREFIX b: <urn:ontokiln:kernel-bench:>
SELECT (COUNT(*) AS ?matchedRows) (SUM(?amount) AS ?amountSum) WHERE {
  ?customer b:region <urn:ontokiln:kernel-bench:region/0> .
  ?order b:customer ?customer .
  ?order b:amount ?amount .
  VALUES ?copy { 1 1 }
}
```

上面使用 PREFIX 简写；harness 使用完整 IRI。独立 oracle 按生成器计算选中 `ceil(records/8)` 个 customer，重复 VALUES 将行数与金额各乘 2；金额为索引 `% 100`，每 25 个入选 customer 的未加倍金额和为 1,200。10K / 100K / 1M 的 `(distinct customers, COUNT, SUM)` 分别为 **(125,250,12000)、(1250,2500,120000)、(12500,25000,1200000)**，不采用引擎自报统计。

全扫描仍独立预期 `(事实数, subject 字符和)` 为 **(10000,421924)、(100000,4219924)、(1000000,42199924)**。multi-target 按每个 focus 的真实 region 分别计算两个 property 的结果，核对完整 coverage、目标集合、路径、Shape 和组件。

[oracle 源码](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/benchmarks/kernel_baseline.py)检查完整语义字段集合，移除诊断 prose 后排序 violations，保留重复而不转为集合。report 的引擎身份仍单独验证，source revision 不参与语义 digest。多目标最后一个 IRI 随规模改变，**应同规模核对前后摘要，不能声称七项跨规模摘要全部相同**。

| 病例 | 成功样本适用范围 | semantic_report_sha256 |
|---|---|---|
| `focus_core_pass` | 三个规模前后 | `62d414d399ca519d4509e3681102b2f4398c93d5c16e4c0da5778052b2f70dbe` |
| `focus_core_fail` | 三个规模前后 | `799268d59063b7332f3bc58a7f7e0a8154d645223eac4ebff574d4540694aa3d` |
| `focus_nonlocal_fail` | 三个规模前后 | `1216fd39bf15d3aeffd9f57ecd39e0a75f4d5d968141da1bfaa199ef57cd245b` |
| `focus_path_fail` | 三个规模前后 | `cc62b2be8dd2097f93979918a3b571bae6a1d00e6077445c5b08fe6a746d85d1` |
| `focus_full_scan_fail` | 三个规模前后 | `9116c77036e76ba1e898ca8eaf494a93abf25edbd159fa13f1e5113ddd26233a` |
| `multi_target_path_fail` | 10K | `dd4503f807f12926755370a8393123ef8881780ab3fd4eb6781f625796ae6f32` |
| `multi_target_path_fail` | 100K | `8fad8dbe3976f39cd362845040a1869972cd4e51502d5da024fec6e1d4f9aa0d` |
| `multi_target_path_fail` | 1M | `14808f4bc23252fccb60cdd83bb61bd1f3dda48d391cb5fcccab2ae975eb9788` |
| `focus_fanout_join_fail` | 10K/100K 前后；1M 仅 after | `4bf58fd685ab06f45abf7f3bf4ab8db8226b38b6c9a8c27a0af5f422537a791e` |

21 个规模/病例组合中的成功样本摘要各自稳定；双方都有成功记录的 **20 组**前后完全一致。1M fanout 只有 after 成功摘要，不能声称该规模已有前后语义比较。kernel 的 12 类 query × 2 规模共 24 组语义摘要、5 类 SHACL oracle 对象也对应一致。harness 用独立图遍历/逐行连接验证公式，并用伪造报告检查漏目标、错来源和重复/缺失违规均不能通过。

## 5. 正式 focused 结果

before focused 于 **06:33:10–06:35:00 UTC** 完成，96 条中为 **92 COMPLETE + 1 次失败预热 + 3 SKIPPED**；after 于 **06:36:33–06:37:16** 完成，96 条全部 COMPLETE。下表是三个成功正式样本中位数，单位为 **ms / API RSS MiB**，变化列只指耗时；全部数据来自本轮重新测量。

| 规模 | 病例 | before ms / MiB | after ms / MiB | 耗时变化 |
|---:|---|---:|---:|---:|
| 10K | Core 通过 | 11.373 / 16.816 | 11.193 / 16.805 | -1.58% |
| 10K | Core 违规 | 11.464 / 16.617 | 11.440 / 16.965 | -0.21% |
| 10K | nonlocal | 11.432 / 18.957 | 11.444 / 19.051 | +0.10% |
| 10K | 独立 path | 11.434 / 18.934 | 11.467 / 18.645 | +0.29% |
| 10K | 全扫描聚合 | 21.383 / 19.781 | 21.576 / 19.988 | +0.91% |
| 10K | 多目标 path | 11.447 / 19.055 | 11.674 / 18.801 | +1.98% |
| 10K | fanout join | 21.652 / 19.590 | 12.702 / 19.656 | -41.33% |
| 100K | Core 通过 | 11.878 / 16.684 | 17.978 / 16.766 | +51.35% |
| 100K | Core 违规 | 11.491 / 16.711 | 11.782 / 16.977 | +2.53% |
| 100K | nonlocal | 21.650 / 22.402 | 11.803 / 18.895 | -45.48% |
| 100K | 独立 path | 32.581 / 22.348 | 12.257 / 18.707 | -62.38% |
| 100K | 全扫描聚合 | 163.442 / 23.914 | 165.449 / 24.031 | +1.23% |
| 100K | 多目标 path | 62.198 / 22.539 | 11.992 / 18.730 | -80.72% |
| 100K | fanout join | 1028.773 / 23.934 | 73.209 / 24.816 | -92.88% |
| 1M | Core 通过 | 11.212 / 16.871 | 11.322 / 17.078 | +0.98% |
| 1M | Core 违规 | 11.261 / 16.801 | 11.812 / 16.973 | +4.89% |
| 1M | nonlocal | 143.354 / 22.711 | 11.498 / 19.023 | -91.98% |
| 1M | 独立 path | 328.404 / 22.688 | 11.645 / 18.777 | -96.45% |
| 1M | 全扫描聚合 | 1533.418 / 23.961 | 1522.313 / 24.062 | -0.72% |
| 1M | 多目标 path | 857.492 / 22.785 | 11.434 / 18.789 | -98.67% |
| 1M | fanout join | 预热预算失败；3 SKIPPED | 558.383 / 36.406 | 不计算倍数 |

1M nonlocal 的范围是 **143.092–163.920 → 11.405–11.599 ms**，独立 path **285.075–378.912 → 11.633–11.824 ms**，多目标 path **822.564–1003.230 → 11.308–11.550 ms**。新版这三项在 10K/100K/1M 保持约 11–12 ms，只能说明这些固定局部病例的实测结果，不能证明一般查询的渐近复杂度或恒定耗时。

fanout 在 100K 为 **1014.310–1085.492 → 72.463–82.662 ms**。1M 前版唯一实际失败发生于预热：端到端 **60.014 s**、API 执行 **60.012 s**、RSS **29.781 MiB**、scratch **4,766 B**、外层 watchdog=false，状态 `RESOURCE_EXHAUSTED / BUDGET_EXHAUSTED`，指标指向时间预算；随后 3 SKIPPED。新版 1M 完成范围 **548.852–609.251 ms**，RSS **36.293–36.609 MiB**；这是完成能力变化，不是与失败前版的速度或峰值内存比。

大左侧成本仍然存在：新版 fanout 的 RSS 随 10K/100K/1M 增长为 **19.656 / 24.816 / 36.406 MiB**，耗时 **12.702 / 73.209 / 558.383 ms**。全扫描聚合前后在 100K 为 **163.442 → 165.449 ms（+1.23%）**，1M **1533.418 → 1522.313（−0.72%）**，保留真正全局计算对照。

反向观测必须保留：100K Core pass **11.878 → 17.978 ms（+51.35%）**，范围 **11.749–12.147 → 12.258–61.031 ms**；1M Core fail **11.261 → 11.812（+4.89%）**。10K multi-target **+1.98%**、path **+0.29%**。不能把回退删掉或未经证据归为噪声；只有三次样本且范围分散，尚不足以独立归因。10K fanout 新版范围 **11.701–22.304 ms**，同样不只报告最好一条。

## 6. kernel 回归、构建及空间

kernel file 每侧 154 条全部 COMPLETE，保留 3 类构建/物化、12 类独立查询、5 类 SHACL。before 于 **06:36:10–06:36:33 UTC**，after 于 **06:37:16–06:37:40** 完成；下表为全部 20 阶段的耗时中位数 ms。全 targetSubjectsOf/targetClass 在 10K/100K 均校验正确的 999/9,999 targets。

| 阶段 | 10K before → after | 100K before → after |
|---|---:|---:|
| pack compile | 26.912 → 20.846（-22.54%） | 210.584 → 184.277（-12.49%） |
| Full compile | 107.656 → 86.294（-19.84%） | 886.264 → 753.914（-14.93%） |
| Full materialize | 117.453 → 106.662（-9.19%） | 1067.284 → 1197.775（+12.23%） |
| pack 点查 | 13.127 → 12.927（-1.53%） | 22.990 → 24.143（+5.02%） |
| pack customer/region join | 13.574 → 12.976（-4.41%） | 23.268 → 23.603（+1.44%） |
| pack 区域聚合 | 13.209 → 13.083（-0.96%） | 33.304 → 43.906（+31.83%） |
| pack 有界路径 | 13.447 → 12.884（-4.19%） | 23.805 → 23.976（+0.72%） |
| pack 全事实计数 | 12.728 → 12.863（+1.06%） | 43.540 → 43.710（+0.39%） |
| compiled 点查 | 13.329 → 12.918（-3.09%） | 43.717 → 55.232（+26.34%） |
| compiled customer/region join | 23.785 → 23.207（-2.43%） | 95.035 → 115.408（+21.44%） |
| compiled 区域聚合 | 64.135 → 54.132（-15.60%） | 511.888 → 546.471（+6.76%） |
| compiled 有界路径 | 13.184 → 13.164（-0.15%） | 43.994 → 44.107（+0.26%） |
| compiled 全事实计数 | 23.611 → 23.021（-2.50%） | 105.559 → 104.652（-0.86%） |
| reasoned 推导谓词计数 | 24.760 → 23.064（-6.85%） | 115.831 → 115.324（-0.44%） |
| reasoned closure join | 13.196 → 12.954（-1.84%） | 54.451 → 64.374（+18.22%） |
| SHACL 全 targetSubjectsOf | 106.025 → 122.227（+15.28%） | 945.762 → 1027.455（+8.64%） |
| SHACL 全显式 targetClass | 95.581 → 105.330（+10.20%） | 1032.528 → 965.472（-6.49%） |
| SHACL Core minCount | 13.213 → 13.126（-0.66%） | 47.665 → 55.148（+15.70%） |
| SHACL compiled 探针 | 13.026 → 13.109（+0.64%） | 55.724 → 54.233（-2.67%） |
| SHACL reasoned 探针 | 12.854 → 13.221（+2.86%） | 54.210 → 54.438（+0.42%） |

主要回退的三样本范围如下。完整小回退和成功样本均保留在 raw/comparison，不能据这些测量把未修改的普通查询、编译或规则物化算法独立归因为改善或恶化。

| 阶段 | before 范围 ms | after 范围 ms | 中位数变化 |
|---|---:|---:|---:|
| 10K SHACL 全 targetSubjectsOf | 105.410–115.782 | 105.249–250.997 | +15.28% |
| 10K SHACL 全显式 targetClass | 95.286–96.216 | 95.326–105.935 | +10.20% |
| 100K pack 区域聚合 | 33.262–33.631 | 33.682–44.385 | +31.83% |
| 100K compiled 点查 | 43.552–103.659 | 46.486–161.546 | +26.34% |
| 100K compiled customer/region join | 94.533–104.757 | 104.866–137.791 | +21.44% |
| 100K reasoned closure join | 53.963–66.872 | 53.934–97.777 | +18.22% |
| 100K SHACL Core minCount | 43.504–53.681 | 43.776–177.988 | +15.70% |
| 100K Full materialize | 1019.569–1081.855 | 1107.474–1229.490 | +12.23% |
| 100K SHACL 全 targetSubjectsOf | 916.440–1046.941 | 977.956–1068.944 | +8.64% |

100K all_orders / class_orders RSS 中位数分别 **37.949 → 38.066 / 38.363 → 38.566 MiB**，Full compile / materialize 为 **45.590 → 45.539 / 53.688 → 53.727 MiB**；12 类普通查询的 API RSS 中位数也有小幅变化，具体在 raw。100K Full compile 耗时范围 **848.060–955.702 → 752.601–959.745 ms**，保留重叠范围而不把单次快值视为确定改进。

focused 组独立编译用同组对照，单位为 **ms / API RSS MiB / scratch MiB**；不得从 kernel 挑选较快构建替换。1M after 编译范围为 **8427.268–9399.503 ms**。

| 规模 | before compile | after compile | 耗时变化 |
|---:|---:|---:|---:|
| 10K | 96.691 / 18.449 / 2.500 | 86.804 / 18.504 / 2.500 | -10.23% |
| 100K | 867.145 / 45.383 / 24.727 | 867.615 / 45.445 / 24.727 | +0.05% |
| 1M | 8939.587 / 166.133 / 377.560 | 8784.265 / 166.094 / 367.333 | -1.74% |

被测 after（ed7）binary 增加 **54,680 字节（0.24%）**。本轮不新增 SQLite 索引或快照格式；**21 对** Full 编译/物化样本的 manifest/content/profile/semantic/source 摘要、manifest 统计和逐文件 logical/allocated 字节对应完全一致。五组产物在重复构建和两个 suite 间也稳定；下表字节与 content digest 前后相同。

| 产物 | SQLite 字节 | 完整目录逻辑字节 | content_digest |
|---|---:|---:|---|
| 10K compiled | 1,409,024 | 2,615,072 | `226cb8b8995d2a809dfedcee02370d72164e2a731c749bd8f0f29319bf5e30b8` |
| 100K compiled | 13,369,344 | 25,397,897 | `d898b49e12a47146f54489f6ef0f39698396658eda56738cc2d01ed5a4dabc97` |
| 1M compiled | 139,964,416 | 260,307,974 | `9dd6df2483cdebdcae27fabea29b6e13771a0e2ded4924dc7d72427090474705` |
| 10K reasoned | 1,867,776 | 3,073,976 | `6cbd4f867ea87ef468553a536e535b13521046f4c520c400f6966d9d7d09f40c` |
| 100K reasoned | 18,309,120 | 30,337,824 | `a6a1350ab866212248c01955e2dbf6de554812992f5c2fe8abae612dbb646fd5` |

6 对 pack 样本的字节和 pack SHA 相同：10K **394,534** logical / **397,312** allocated，100K **3,886,533 / 3,887,104**；除预期 kernel 身份外，pack identity 相同。完整 manifest/pack 摘要见各 raw。微量 request scratch 差异包含日志/响应字节，不能简单解释为数据结构空间差异；session scratch 不含已挂载数据库。

## 7. 非计时访问量诊断与语义回归

[SQLite store 回归](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/tests/shacl/sqlite_store.rs)构造固定局部分支和共享相同谓词的无关背景。progress handler 每 100 条 VM 指令回调，记录值是回调计数乘 100；这是粒度为 100 的访问量诊断，不是 API 时间，也不是精确到单条指令的 profile。

| 已观察的独立诊断 | 32 背景 | 8,192 背景 | 解释 |
|---|---:|---:|---|
| 旧源码 three-quad BGP | 900 | 302,600 | 8,192 规模超过原定 20,000 门槛而失败，证实测试可识别全谓词分支扫描 |
| 新版 three-quad BGP / 三跳 path | 400 | 400 | 相同固定局部结果、含重复路径的正确 bag |
| 新版 inverse 两跳 path | 100 | 100 | 反向访问控制 |
| 新版所有共享键已被外层绑定 | 3,000（单个 64×32 用例） | 不适用 | 保留一次右扫描，输出 2,048 个兼容组合 |

访问量记录与来源见 [query-work-regression.json](shacl-bind-joins-20261113/query-work-regression.json)，旧源码失败保留于 [baseline-regression.log](shacl-bind-joins-20261113/baseline-regression.log)。不能把失败门槛或者 VM 比率写成 API 加速倍数；最终源码的门槛未放松，小而固定的诊断也不证明所有业务查询恒定耗时。

新增 4 组[独立语义预期](https://github.com/Codefor/ontokiln/blob/ed7b90d8feb2c00b87a4dc4981a2c36d67325355/tests/shacl/sqlite_validation.rs)同时在旧源码通过，记录见 [baseline-semantics.log](shacl-bind-joins-20261113/baseline-semantics.log)：重复 VALUES/菱形路径/RHS 多匹配；UNDEF、OPTIONAL、UNION；全局 LIMIT 子查询及私有变量；FILTER EXISTS 内 BIND 重写与原输入兼容。它们检查明确答案，避免仅因 memory/SQLite 共用求值器而发生同错同过。

## 8. pack 共享求值器的额外回归

vendor spareval 由 pack 与 Full 路径共用，因此另用历史两模板 `intersection` / `exists` 检查普通 pack 行为。完整 [indexed-regression.json](shacl-bind-joins-20261113/indexed-regression.json)在 **06:39:38–06:47:59 UTC** 完成：**88/88 HTTP 请求 COMPLETE、0 timeout/rejected**，分别为 8 次预热、72 次正式测量、8 次独立 counters。每个成功请求均 HTTP 200、oracle_checked=true；这组与 Full/kernel 的 500 条记录分开，仍是 **b8/ed7** 对照，不是 6d7 的实测或 Full 100M 容量声明。

冻结 [indexed-regression.py](shacl-bind-joins-20261113/indexed-regression.py) SHA 为 **`64c153fc2764c11b9efd7d9a1da738b0de30a02321e9ec1bcc4530259a40a37e`**，复用仓库 fixture/查询生成器。两规模为 **250,000 panels / 1,113,595 triples** 与 **22,471,664 panels / 100,000,000 triples**；每 binary、每模板 **1 预热 + 9 测量 + 1 counters**。双方共享由 before 构建的一份 pack，逐一核验 binary/runtime identity、pack SHA 和事实数。

HTTP 每请求显式设置 `max_visits=200000000`、deadline 5 s，串行、1 线程；计时覆盖发出请求至读完响应体。运行顺序固定为 before 后 after、intersection 后 exists；fixture 和 TMPDIR 实际在 **`/workspace` overlay**，与 Full 正式组 `/tmp` tmpfs 不同，OS cache 未清空。下表仅取九次普通正式请求，counters 不混入延迟；单位为 ms，中位数后附完整范围。

| 事实数 | 模板 | before 中位数（范围） | after 中位数（范围） | 中位数变化 |
|---:|---|---:|---:|---:|
| 1,113,595 | `intersection` | 41.676（39.217–48.506） | 39.683（36.941–46.837） | -4.78% |
| 1,113,595 | `exists` | 42.333（41.326–54.830） | 46.076（41.528–62.908） | +8.84% |
| 100,000,000 | `intersection` | 3511.168（3241.269–3672.703） | 3407.212（3273.396–3528.243） | -2.96% |
| 100,000,000 | `exists` | 3568.131（3493.880–3802.557） | 3701.365（3537.486–4506.531） | +3.73% |

EXISTS 在小控制与 100M 分别回退 **8.84% / 3.73%**，必须与 intersection 的小幅下降同时保留。范围存在重叠且前后执行顺序固定，这组主要验证完成能力、结果和访问量；不从单组中位数变化推断因果或宣称所有 pack 查询提速。

独立预期与八次 counters 全部匹配：小控制 COUNT **12,500**、候选行 **75,000**（POS 62,500 + SPO 12,500）；100M COUNT **1,123,584**、候选行 **6,741,500**（POS 5,617,916 + SPO 1,123,584）。两模板、前后版本均相同，OSP 为 0。`peak_heap_bytes` 在 intersection / exists 分别为 **20,437 / 21,720 B**，属于查询计数器的追踪范围，不能称为服务 RSS、完整堆占用或 100M 数据的总内存。

| 数据规模 | before 单次 pack 编译 s | 编译峰值 RSS GiB | 共享 pack 字节 | before / after 启动至 READY s，各一次 |
|---:|---:|---:|---:|---:|
| 1,113,595 | 2.635 | 0.069 | 35,759,200 | 0.186 / 0.133 |
| 100,000,000 | 282.297 | 5.504 | 3,242,956,152 | 17.638 / 16.109 |

100M 编译的采样峰值与 wait4 峰值均为 **5,910,085,632 B（5.504 GiB）**，耗时 **282.296863 s**；7.5 GiB 限额采用 20 ms RSS/HWM 轮询，属于采样后终止/回收机制，不是严格硬限。每份 pack 只用 before 构建一次，不能据此比较编译速度。100M 服务 READY RSS 为 before/after **3.027 / 3.028 GiB**，与上面的查询追踪 heap 分开解释；不要相加共享 mmap 页面或把启动单观测当成冷启动分布。

100M 共享 pack SHA 为 **`0253522dee38e1e65f4febc67560835181289f0ff26be20cb71b72d853cf2f6b`**；小控制为 **`3985547f00b73af5e8c9e18d7540e46c83c61da9c1f928525f1d34eddcc97019`**。记录的两次编译和四个服务进程全部 `process_reaped=true`；服务退出 `-15` 是脚本结束测量后的主动终止，不计为请求失败。

另保留 [100-panel smoke](shacl-bind-joins-20261113/indexed-smoke.json)：1,540 triples，预期 COUNT=5/candidate_rows=30，44 请求全成功；[before](shacl-bind-joins-20261113/before-focused-smoke.json) / [after](shacl-bind-joins-20261113/after-focused-smoke.json) 的 100-fact focused smoke 各 16 条全 COMPLETE，multi-target 为 2 targets / 3 violations。上述 smoke 不混入正式样本。

## 9. 被测版本验证、最终集成与 CI

`round4-validation.json` 的 `tested_staged_tree` 为 **`6f0a61337de3c1d55de108e38e1c70ad5a3c7593`**，对应本地 `47bd8bb2d352228f52dc2d2e6d6609dd0281ce25`。随后保留并行业务库自动发现工具，重放到 `c18a3cb473ab510fc2e508c12eb75da05e1b7d9f`，再创建远端 ed7；c18/ed7 的 tree 均为 **`acf9698ea116f9e65be3a60de627f47e5d968b67`**。两代码树之间仅变更业务基准工具/测试/说明，参与引擎 fingerprint 的输入完全相同；不能把两个不同完整 tree 写成同一个。

隔离旧源码回归使用共用 Cargo target 后，主工作区门禁一度观察到与旧实现相同的 **900 / 302,600** 工作量。旧源码构建日志记录了 baseline worktree 中 spareval 的编译；按构建缓存混用方向清理受影响的 path dependency 及消费者后，未修改源码即通过 43 项 SHACL 专题及 ed7 来源代码完整 gates。失败与排查材料保留于 [shared-target-attempt](shacl-bind-joins-20261113/shared-target-attempt/)；其中 `dep-info-roots.json` 的 source_roots 为空，不构成旧 rlib 来源路径的确证。这里保留观测及处理经过，不将这次构建/验证异常当作生产语义失败，也不把缓存混用的推断写成已独立证明的根因。

| 检查 / 测量角色 | 已观察状态 |
|---|---|
| ed7 来源代码：fmt / Clippy / workspace / all-features / scenarios | 全部 exit 0；scenario 14/14 |
| ed7 来源代码：原生库 | 55：27 store、16 差分、12 其他，全部通过 |
| ed7 来源代码：API / session / SHACL / Full | 35 / 8 / 11 / 15，全部通过 |
| ed7 来源代码：Python harness / comparator / 业务工具 | 业务工具合入前 86，合入后 100/100；冻结 harness 未再修改，不指随后 796a 的 merge 门禁 |
| 正式 after 与两比较器 | 250 条全 COMPLETE；两组 comparable=true、0 issues |
| 额外 pack 小控制与 100M 两模板 | 88 请求全 COMPLETE，结果/candidate oracle 匹配，0 timeout/rejected |
| 6d7 集成代码的独立语义评审 | 未发现阻断项；不替代实际构建与测试 |
| 6d7：corpus / fmt / Clippy / workspace / all-features / scenarios / Python | 全 PASS、exit 0；root 55、API 35、session 9、SHACL 11、legacy Full 15；scenario 14/14、Python 100/100 |
| 6d7：release 与 100-fact focused smoke | release 构建 PASS；16/16 COMPLETE，全部 oracle.checked=true；独立于正式 500 条 |
| 远端 CI | 报告提交的远端 CI 以 [PR #1](https://github.com/Codefor/ontokiln/pull/1) 对应 head 运行记录为准；本地验证不替代远端 CI |

最终集成证据为 [integration-validation.json](shacl-bind-joins-20261113/integration-validation.json)与 [integration-focused-smoke.json](shacl-bind-joins-20261113/integration-focused-smoke.json)，均记录 clean 6d7。编排先等待 pack 完成，实际检查从 **06:47:59 UTC** 开始；workspace / all-features 分别为 **50.148 / 50.006 s**，最终 release/smoke 于 **06:50:25** 完成。session 从原来 8 组增加至 9 组，覆盖兼容读取；100-fact smoke 使用冻结的同一七病例 harness，多目标为 2 targets / 3 violations。

构建、验证树、二进制身份、日志 SHA 以及各种测量角色汇总于 [build-receipt.json](shacl-bind-joins-20261113/build-receipt.json)，64,577 字节，SHA **`9c9fc8bb91da8101953d2016115a64d9e70953d3e33e6c20c642bba43b537501`**。ed7 的 gates、merge 的 gates/smoke 和远端 CI 分别记录；后两者不追溯改变已有 b8/ed7 raw 的身份或计量范围。

附件日志逐字保留，包括 Cargo/test 输出末尾的空行。默认 `git diff --check` 对其中六份日志报告 `new blank line at EOF`；这些原始字节与 receipt 中的 SHA 对应，未为格式检查改写。排除原始 `.log` 附件后，其余本轮变更通过 `git diff --check`。

## 10. 兼容、冷启动及复现

每请求隔离、预算回收、API v1 字段、只读 union/strict RDF 条件、旧索引 fallback、`shapes_from_dataset` 完整 Shapes 读取以及未知/宽松策略全图校验继续保留。身份准入须区分被测版本与最终集成版本：

| 版本 / 操作 | 原生 `kernel-manifest-v2` 的身份准入 |
|---|---|
| 本文被测 b8/ed7 对原生 v2 的读取 | 仍要求完整 kernel 身份精确匹配；正式 API 基准实际编译的是常规 v1 产物 |
| 6d7 源码已集成的 796a 兼容读取 | 仅允许 `source_revision` 与 `kernel_sha256` 两项不同，且分别为合法格式的 40/64 位小写十六进制；其它 kernel 字段、产物格式和语义版本必须匹配 |
| 构建与物化 | 继续 exact producer admission，兼容读取规则不放宽这两类操作 |

796a 的 [`verify_read_manifest`](https://github.com/Codefor/ontokiln/blob/796ac314ae7f6b8d6fb8f8e3cb0b4e06d90051c2/src/full_artifact.rs)用于普通 API 数据集读取与 Full snapshot 挂载。它保留产物原始 producer metadata 和 manifest 字节，仍执行 inventory、输入、semantic fingerprint 与完成状态检查，不通过改写 manifest 冒充当前生产者。[兼容读取说明](https://github.com/Codefor/ontokiln/blob/796ac314ae7f6b8d6fb8f8e3cb0b4e06d90051c2/docs/session-v1.md)

6d7 的独立完整门禁已通过，新增 session 回归验证兼容读取，16 条 focused smoke 验证合入后的 SHACL 病例；这些验证不替代正式性能重测。其它不满足格式/语义/身份要求的产物仍按既有规则拒绝。无需一律重建兼容的只读产物，也不承诺任意旧产物跨 kernel 无条件可用。

| mount，单次观测 | before READY / 内部 ms | after READY / 内部 ms | 双方私有复制字节 |
|---:|---:|---:|---:|
| 10K | 5.923 / 4.097 | 5.630 / 3.708 | 1,409,024 |
| 100K | 42.281 / 40.275 | 38.689 / 36.410 | 13,369,344 |
| 1M | 397.169 / 389.025 | 336.795 / 333.666 | 139,964,416 |

成功 session 读取中 before **80 条**、after **84 条**（含预热）均 `snapshot_reused=true`、复制字节为 0。常驻实例启动仍核验并复制快照，每次请求仍启动 worker；上述各一次、tmpfs、未清 OS cache 的挂载观测不是冷盘分布，不计算稳定提速率，也不说明冷启动消失。

复现时固定两份源码构建 release，并对双方运行同一冻结 harness；构建在计时之外，设置相同 Rust PATH/TMPDIR。focused 参数为 `--suite focused-shacl --read-transport session --sizes 10000 100000 1000000 --build-samples 3 --validation-samples 3 --query-samples 9`；kernel 参数为 `--suite kernel --read-transport file --sizes 10000 100000 --build-samples 3 --validation-samples 3 --query-samples 3`。两组统一 `--memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 --threads 1 --build-profile release --build-features default --snapshot-guard manifest`。

正式 comparator 已分别核对同 suite/transport 的完整 raw，保留所有失败与 SKIPPED。任何额外放大预算测试都应双方对称重测并另存，不能覆盖失败或拼接其它轮次结果。

| 已完成原始证据 | SHA-256 |
|---|---|
| before focused | `057df205bb2efc153454e087d01f9223e57493267f017f92d9d856a27f81dd85` |
| before focused 100-fact smoke | `b024cbf97278109b87fbb0837ebe569828371cca1fd55871b1a97d881b245e1d` |
| after focused 100-fact smoke | `6c5fb1aeeb09e27a70850ada24986ddaecede27d355bb4eddf7b781d71e2fc52` |
| pack 100-panel smoke | `132e414869720861a7603deaaa6a70ed014508c0ec2af896e1a9d6219cce2814` |
| local validation | `669ba33ac0e4b1a6fd2af8af7b1a5a974a330eea8be73fb247c2d7eded718fe6` |
| after focused | `6e55cc175b4f19cbf22bd4acbaae5f76b8fd74ee4d91313613677dc5004ec429` |
| before kernel | `b994669fd716858e854b9648c08c24feaf673d990b146a4e0d946fe70ecc35be` |
| after kernel | `a15f34d95ca12f1be24b8318e3f15f6d5001587b5ac2df77b9ddfa3d35fe299a` |
| focused comparison | `fdc00a3c3ca1bb95a4488bce730751ffcfb6688bd4b2b855bb26e93f2ec72cf9` |
| kernel comparison | `1099dac5903393b086790c20c7f070485568ec74616e9c87a4d716c914cdc947` |
| formal audit | `980633c433e5e93d2fe46bb09f06a6ecffef392bd3465795af4ef10cff58bd57` |
| pack 小控制与 100M 完整 raw | `c7b33d003fbdba88358037cba47d7a0f23bf488bc4898822c88237df61b22144` |
| integration validation | `35cf772a340e2c4c699c0cf2f99cbff2ca248300f2806555a5cc21106ee25665` |
| integration focused smoke | `6078236dc89cdb1f1a055bf7b7d5cd672f1fd8ddcc762be5d4246fc20be23094` |
| build receipt | `9c9fc8bb91da8101953d2016115a64d9e70953d3e33e6c20c642bba43b537501` |

本报告只覆盖保留的合成工作负载与真实预算终态，不外推完整 SHACL、生产知识图谱、多租户并发、HTTP 吞吐或 Full 亿级容量。新物理连接的适用范围与全局计算成本应分开解释。
