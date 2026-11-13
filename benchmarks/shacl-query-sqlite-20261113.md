# SHACL 复杂路径与 SPARQL 的 SQLite 查询实测（2026-11-13）

本轮让 Full SHACL 的复杂路径、SHACL-SPARQL、查询 target 和自定义 validator 直接读取已核验的 SQLite 快照，避免先创建完整内存查询图。相同 512 MiB / 60 s 预算下，100K 背景的 nonlocal 从 **823.077 降至 34.013 ms（24.20 倍）**，独立两跳 path 从 **801.615 降至 32.256 ms（24.85 倍）**；两项 API 峰值 RSS 中位数由约 154 MiB 降至约 24 MiB。

1M 背景下，前版 nonlocal、path、全扫描聚合均在预热时耗尽 RSS 预算；新版均完成预热及三次正式请求，中位数分别为 **143.333 / 281.027 / 1635.086 ms**，RSS 为 **23.988 / 24.211 / 25.379 MiB**。失败的前版没有成功基准，不计算这三项的前后倍数。固定 focus 的延迟仍随背景规模增长，不能据此声称恒定耗时。

收益集中于原来需要构造查询图的路径。普通 Core 基本保持约 11–12 ms，RSS 略升；kernel 中部分查询、构建和挂载观测变慢，均保留于下文。初版候选的分页退化单列诊断，不与最终结果拼接。

## 1. 正式测量与来源身份

| 测量组 | 正式前版 | 正式新版 | 比较结果 |
|---|---|---|---|
| focused session，10K / 100K / 1M | [before-focused-session.json](shacl-query-sqlite-20261113/before-focused-session.json) | [after-focused-session.json](shacl-query-sqlite-20261113/after-focused-session.json) | [JSON](shacl-query-sqlite-20261113/focused-comparison.json) · [Markdown](shacl-query-sqlite-20261113/focused-comparison.md) |
| kernel file，10K / 100K | [before-kernel-file.json](shacl-query-sqlite-20261113/before-kernel-file.json) | [after-kernel-file.json](shacl-query-sqlite-20261113/after-kernel-file.json) | [JSON](shacl-query-sqlite-20261113/kernel-comparison.json) · [Markdown](shacl-query-sqlite-20261113/kernel-comparison.md) |

| 身份 | 前版 | 最终新版 |
|---|---|---|
| 二进制源码提交 | [`f5b8c5f05a3de9313191e4a9ffeebab16df16648`](https://github.com/Codefor/ontokiln/commit/f5b8c5f05a3de9313191e4a9ffeebab16df16648) | [`b8ecf8046a286345926077318a78dde7541fb98b`](https://github.com/Codefor/ontokiln/commit/b8ecf8046a286345926077318a78dde7541fb98b) |
| 二进制 SHA-256 | `4aca58f157d080636673dece73179ccd32222947142014f1cdd7416d94801064` | `83680cfeca52cdd201ba02f9c40fa5bca19ad18a1a9443942054df83999d78c1` |
| kernel SHA-256 | `faf847360784cfd5c8fe6279219eeca3119624bae2f5217d0d7c872c833a1c39` | `ccb28441555497f8cd541412cce5a02eca371d4853b551c5674de2208d9c8b78` |
| 完整 Git tree | `d52cd84ebc896ac33f38942159fcae9e2ca89eeb` | `3347b0b9f9e0fb75286df19592b5254bdf6a1b11` |

前版复用[上一轮已记录的 release 二进制](shacl-sqlite-20261113/build-receipt.json)。报告提交 `45a7dbe63f0e86d78f1f9b5a3c00089134fbf9a7` 相对 f5 只增加报告，内核源码相同。正式 before 运行 harness 时仓库已到 `83e7854…`，其中 kernel 组记录 dirty；这些是 harness 工作区状态，不替代保存二进制的身份。最终 after 两份均记录 clean `b8ecf804…`。

最终门禁在提交前的同代码树工作区执行，本地等价提交为 `e1421dec3fc7f10f8866abb90e6602d700d6405a`，其 tree 与远端 API 创建的 b8 完全一致；初版 `83e7854` 作为父提交保留。源码提交、实际被测 binary 和后续报告提交分别记录于 [build-receipt.json](shacl-query-sqlite-20261113/build-receipt.json)。

正式 before 为补齐工具链 PATH 后的完整重测：focused 05:27:39–05:28:34 UTC，kernel 05:28:34–05:28:58；after 分别为 05:42:15–05:42:55、05:42:55–05:43:19。旧 before 与候选缺少 `rustc_on_path`，单列保存，没有回填 raw 元数据。

## 2. 实现、语义及缓存边界

前版 [`PreparedQueries`](https://github.com/Codefor/ontokiln/blob/f5b8c5f05a3de9313191e4a9ffeebab16df16648/src/rdf_graph.rs)会调用 `load_graph()?.query_dataset()`。新版在[SQLite 适配器](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/src/shacl_sqlite.rs)实现现有求值器的 `QueryableDataset`，由[查询入口](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/src/rdf_graph.rs)按绑定的 S/P/O 模式读取。适用前提是快照已核验，输入 manifest 能确认严格 RDF 解析策略；没有按合成数据的业务词汇分支。

- 数据仍是所有存储图、显式与推导事实的去重 SPO union，作为默认图查询；不暴露原始具名图，focus 不限制可见数据。
- 每个模式读取器按 256 条键分页；ID、原生词项与查询词项缓存各有 4,096 条、估算 4 MiB 的上限。严格 N-Triples 解码、字典 ID 和外部表达式/参数词项保留 RDF 词项身份，缺少字典 ID 不等于未绑定。
- ASK / SELECT 的预绑定和词项语义保持既有规则；执行及结果迭代后检查底层 sticky error，避免 FILTER / EXISTS 抑制存储错误后错误返回空集或 conforms=true。预算错误保持原类别。
- 会话复用已核验快照；worker、SQLite 连接/page cache、词项缓存、查询计划与结果均属于单次请求。最近一个完整 Core 邻域最多 4,096 个对象、估算 1 MiB，超限或未读完的邻域不当成完整缓存。

分页 SQL 将已绑定词项的单行字典查找放在 facts 前，其余字典引用放在 facts 后，保留三处引用等式和 DISTINCT；后续游标仅比较未固定后缀。可选 INDEXED BY 只用于已核验归属 facts、非 partial、前三 key 列正确且为 ASC/BINARY 的既有 SPO/POS/OSP 索引；缺失或定义不符则用通用 SQL。没有新增索引或采用人工 MIN 范围方案，具体计划证据见第 7 节。

[API v1 合同](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/docs/api-v1.md)字段、单次 API 和原有显式 SHACL 目标语义保持不变；本轮不新增隐式 class target。

## 3. 环境、计时和统计

| 项目 | 配置 / 口径 |
|---|---|
| 机器 | Linux 6.18.57 x86_64、glibc 2.41；AMD EPYC 9V74；9 个可见逻辑 CPU；约 9.73 GiB 内存 |
| 临时文件系统 | 实际为 **`/tmp` tmpfs**，不是 `/workspace` overlay 的磁盘成绩 |
| 构建与依赖 | release、default features、锁定依赖；正式四份均记录 Rust 1.95.0 / LLVM 22.1.2、SQLite 3.54.1 |
| 并发与预算 | 串行，Rayon 1 线程、Tokio 1 worker；512 MiB RSS、4 GiB scratch、60 s；预算未放松 |
| 输入 | 10K / 100K / 1M 显式事实，999 / 9,999 / 99,999 条合成记录；前后 input、limits、stage metadata 一致 |
| 样本 | 每项读取预热 1 次、正式 3 次；编译/物化独立 3 次；每个 session mount 只观测 1 次 |
| 缓存与统计 | 不清 OS 缓存；生成/摘要/编译会预热文件页；成功正式样本中位数及范围，百分比按未舍入数据计算 |

session 从写出 JSONL 请求到收到响应行计时，排除一次性挂载；file 从创建进程到退出、读取结果计时，包含每次快照准备。两组只在相同 transport 内比较；focused 的 compile 仍走独立文件 API。pack compile 是旧独立构建 CLI，RSS/scratch 不由 API 预算强制，仅外层超时监控，不能冒充 API 预算结果。

API RSS 是监督进程/worker 的预算计量，受采样和共享页影响，不是 PSS；不能与监督进程单列 RSS 或 wait4 RSS 相加。tmpfs 使用主机内存，scratch 字节不等于物理磁盘 I/O。三次正式样本只支持描述性比较，不提供统计显著性、P95/P99 或生产 SLO。

四份正式报告的 harness SHA 均为 `384ea1c24545ae2419d882cb038bf80a00cef19a624311dfe779354c4167fc19`，工具链、Cargo.lock、方法和同规模输入匹配；两组比较器均 **`comparable=true`、`issues=[]`、`methodology_changed=false`**。可比不等于所有阶段都提速，失败阶段和单次 mount 不输出速度百分比。

## 4. 五个工作负载与独立 oracle

focused 固定 order `000000008`、一个 selected Shape 和一个 focus，局部事实随规模保持不变；额外 Shapes 只加到 focused，普通 kernel 输入字节不变。

| 用例 | 独立预期 |
|---|---|
| `focus_core_pass` | 原生 customer / amount Core 约束通过，零违规，作为上一轮优化路径的对照 |
| `focus_core_fail` | amount 的 minCount 2 必须产生唯一正确的原生违规 |
| `focus_nonlocal_fail` | 两跳 customer/region 正确；SPARQL 找到无关 order 0 的语言标签并产生唯一违规 |
| `focus_path_fail` | 独立两跳路径，正确 region 的 hasValue 通过，错误 region 的 hasValue 产生唯一正确来源的违规 |
| `focus_full_scan_fail` | 全 union 的 COUNT 和逐行 subject 长度 SUM 同时等于预期，产生唯一 SPARQL 违规 |

nonlocal 的谓词和 `"Order 0"@en` 对象已绑定，可选择性访问，不等同于全图扫描。全扫描显式执行下列聚合，输出只有一行，SUM 要求逐输入求值，不能仅靠 COUNT 统计捷径通过：

```sparql
SELECT $this WHERE {
  { SELECT (COUNT(*) AS ?factCount) (SUM(STRLEN(STR(?s))) AS ?subjectChars)
    WHERE { ?s ?p ?o } }
  FILTER (?factCount = EXPECTED_FACTS && ?subjectChars = EXPECTED_CHARACTERS)
}
```

harness 按固定 schema 以及每记录 6 条 order、4 条 customer 主语独立计算预期，不读取引擎统计：10K `(10000,421924)`，100K `(100000,4219924)`，1M `(1000000,42199924)`。oracle 检查 conforms、checked_shapes、target_count、完整 coverage 和 violations 字段集合，包括 focus、path、来源 Shape 与组件；只移除诊断 message 和 build identity 后生成语义摘要。[固定 harness 与 oracle](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/benchmarks/kernel_baseline.py)

前后所有成功 focused 样本按用例具有相同摘要，跨规模也一致；漏执行约束、零目标、错误来源或不完整路径不能通过。kernel 的 12 类独立查询同规模 semantic_sha256 亦全部一致。

| focused 用例 | 所有成功样本的 semantic_report_sha256 |
|---|---|
| `focus_core_pass` | `62d414d399ca519d4509e3681102b2f4398c93d5c16e4c0da5778052b2f70dbe` |
| `focus_core_fail` | `799268d59063b7332f3bc58a7f7e0a8154d645223eac4ebff574d4540694aa3d` |
| `focus_nonlocal_fail` | `1216fd39bf15d3aeffd9f57ecd39e0a75f4d5d968141da1bfaa199ef57cd245b` |
| `focus_path_fail` | `cc62b2be8dd2097f93979918a3b571bae6a1d00e6077445c5b08fe6a746d85d1` |
| `focus_full_scan_fail` | `9116c77036e76ba1e898ca8eaf494a93abf25edbd159fa13f1e5113ddd26233a` |

## 5. 正式 focused 结果

单位为 **ms / API 峰值 RSS MiB**；每格为三个成功正式样本中位数，变化列仅指耗时。前版取最新完整重测，不使用 initial 或候选值替换。

| 规模 | 用例 | 前版 ms / MiB | 新版 ms / MiB | 耗时变化 |
|---:|---|---:|---:|---:|
| 10K | Core 通过 | 11.315 / 16.652 | 11.569 / 18.047 | +2.25% |
| 10K | Core 违规 | 11.398 / 16.609 | 11.676 / 18.023 | +2.44% |
| 10K | nonlocal | 82.646 / 31.633 | 11.505 / 20.520 | -86.08% |
| 10K | 独立 path | 61.963 / 31.793 | 12.443 / 20.441 | -79.92% |
| 10K | 全扫描聚合 | 62.230 / 31.816 | 21.584 / 21.352 | -65.32% |
| 100K | Core 通过 | 11.155 / 16.656 | 11.352 / 17.812 | +1.77% |
| 100K | Core 违规 | 11.388 / 16.828 | 11.408 / 17.848 | +0.18% |
| 100K | nonlocal | 823.077 / 153.438 | 34.013 / 24.148 | -95.87% |
| 100K | 独立 path | 801.615 / 153.758 | 32.256 / 23.977 | -95.98% |
| 100K | 全扫描聚合 | 822.955 / 153.594 | 174.357 / 25.410 | -78.81% |
| 1M | Core 通过 | 11.817 / 16.855 | 11.815 / 17.723 | -0.01% |
| 1M | Core 违规 | 11.564 / 16.824 | 11.431 / 17.754 | -1.15% |
| 1M | nonlocal | 预热预算失败；3 SKIPPED | 143.333 / 23.988 | 不计算倍数 |
| 1M | 独立 path | 预热预算失败；3 SKIPPED | 281.027 / 24.211 | 不计算倍数 |
| 1M | 全扫描聚合 | 预热预算失败；3 SKIPPED | 1635.086 / 25.379 | 不计算倍数 |

100K 的 nonlocal 范围为 **701.151–1022.457 → 21.866–106.701 ms**，path 为 **773.031–868.267 → 31.793–52.546 ms**；前后有明确间隔，但新版 nonlocal 三次分散较大。全扫描为 **761.430–933.518 → 163.318–174.519 ms**，中位数约 4.72 倍改善。

1M 新版 nonlocal / path / 全扫描范围分别为 **143.292–153.263 / 275.918–502.941 / 1349.746–1654.594 ms**，RSS 范围分别为 **23.891–24.062 / 23.805–24.426 / 25.277–25.406 MiB**。前版三次实际失败为 **4.699 / 4.337 / 4.178 s**，RSS 均超过 512 MiB，状态 `RESOURCE_EXHAUSTED / BUDGET_EXHAUSTED`，随后各 3 SKIPPED；它们不进入成功中位数。

100K → 1M 时，新版 nonlocal / path / 全扫描中位数分别增长 **4.21 / 8.71 / 9.38 倍**。全扫描仍支付随输入增长的计算量；选择性案例也没有达到恒定延迟。普通 Core 耗时变化为约 −1.15% 至 +2.44%，RSS 从约 16.6–16.9 升至 17.7–18.0 MiB。10K path 新版范围 11.540–22.080 ms，不能只报告最好一条。

## 6. kernel 回归、构建与空间

完整 file 套件保留 Core、全 targets、编译、物化和独立 SPARQL。全目标校验在 10K / 100K 分别检查 999 / 9,999 个目标；全部样本通过 oracle。下表为耗时中位数 ms，括号为变化。

| 阶段 | 10K 前版 → 新版 | 100K 前版 → 新版 |
|---|---:|---:|
| pack compile | 23.601 → 22.340（-5.34%） | 196.118 → 222.579（+13.49%） |
| Full compile | 87.169 → 85.768（-1.61%） | 730.598 → 732.251（+0.23%） |
| Full materialize | 107.821 → 106.885（-0.87%） | 1084.464 → 999.288（-7.85%） |
| pack 点查 | 13.076 → 13.022（-0.41%） | 24.328 → 13.820（-43.19%） |
| pack customer/region join | 13.358 → 12.822（-4.01%） | 23.567 → 40.948（+73.75%） |
| pack 区域聚合 | 13.318 → 12.631（-5.16%） | 34.218 → 34.034（-0.54%） |
| pack 有界路径 | 13.340 → 12.735（-4.53%） | 23.096 → 13.131（-43.15%） |
| pack 全事实计数 | 13.015 → 12.685（-2.54%） | 43.874 → 33.795（-22.97%） |
| compiled 点查 | 13.050 → 12.491（-4.28%） | 44.041 → 43.437（-1.37%） |
| compiled customer/region join | 23.889 → 28.975（+21.29%） | 105.744 → 94.421（-10.71%） |
| compiled 区域聚合 | 53.798 → 58.347（+8.46%） | 592.786 → 562.963（-5.03%） |
| compiled 有界路径 | 13.358 → 14.005（+4.84%） | 44.151 → 54.199（+22.76%） |
| compiled 全事实计数 | 23.168 → 23.225（+0.25%） | 95.313 → 94.241（-1.13%） |
| reasoned 推导谓词计数 | 23.285 → 23.341（+0.24%） | 105.140 → 135.689（+29.06%） |
| reasoned closure join | 13.202 → 13.350（+1.12%） | 54.146 → 54.788（+1.19%） |
| SHACL 全 targetSubjectsOf | 119.403 → 95.790（-19.78%） | 856.969 → 912.050（+6.43%） |
| SHACL 全显式 targetClass | 95.932 → 96.453（+0.54%） | 947.175 → 965.826（+1.97%） |
| SHACL Core minCount | 13.060 → 13.089（+0.22%） | 43.941 → 44.274（+0.76%） |
| SHACL compiled 探针 | 13.252 → 13.003（-1.88%） | 43.939 → 43.660（-0.64%） |
| SHACL reasoned 探针 | 13.012 → 13.095（+0.64%） | 53.920 → 53.826（-0.18%） |

回退及样本范围必须一并看：100K pack join **23.312–24.054 → 24.026–50.747 ms（+73.75%）**，reasoned 推导谓词计数 **104.894–125.042 → 104.867–225.060（+29.06%）**，compiled path **43.836–54.409 → 43.598–54.345（+22.76%）**；10K compiled join **23.688–24.375 → 22.933–83.095（+21.29%）**、区域聚合 **53.676–64.397 → 54.824–74.606（+8.46%）**。这些是本次端到端观测，不独立归因于未修改的普通查询算法。

100K all_orders 为 **856.723–1039.637 → 876.280–967.231 ms（+6.43%）**，class_orders 为 **917.135–1046.507 → 891.962–1033.621（+1.97%）**；RSS 中位数分别 **37.875 → 39.055、38.562 → 39.293 MiB**。100K pack compile **193.649–209.994 → 185.418–364.099 ms（+13.49%）**，Full compile **709.435–933.991 → 731.882–910.058（+0.23%）**，materialize **1066.368–1205.618 → 937.382–1257.160（−7.85%）**；Full compile/materialize RSS **45.434 → 46.684 / 53.391 → 55.078 MiB**。完整逐样本范围保留在 kernel 附件，小差异不解释为稳定收益。

focused 组独立编译结果如下，不能用这一组较快的样本替换 kernel 构建数据。单位为 **ms / API RSS MiB / scratch MiB**；100K 新版耗时范围 **782.076–1003.013 ms**，1M 为 **7915.013–9334.551 ms**。

| 规模 | 前版编译 | 新版编译 | 耗时变化 |
|---:|---:|---:|---:|
| 10K | 96.557 / 18.363 / 2.500 | 107.488 / 19.547 / 2.500 | +11.32% |
| 100K | 810.128 / 45.242 / 24.727 | 950.919 / 46.766 / 24.727 | +17.38% |
| 1M | 8691.521 / 166.133 / 375.146 | 7935.001 / 167.059 / 363.892 | -8.70% |

最终 release binary 为 **22,995,720 字节**，前版 **22,123,576 字节**，增加 **872,144 字节（3.94%）**。同规模 Full 产物的前后各三次构建均核对一致：manifest SHA、content digest、semantic fingerprint、source tree digest，以及逐文件逻辑/allocated 字节；没有用增加索引空间换取本轮收益。下表字节和 content digest 前后相同。

| 产物 | SQLite 字节 | 完整目录逻辑字节 | content_digest |
|---|---:|---:|---|
| 10K compiled | 1,409,024 | 2,615,072 | `226cb8b8995d2a809dfedcee02370d72164e2a731c749bd8f0f29319bf5e30b8` |
| 100K compiled | 13,369,344 | 25,397,897 | `d898b49e12a47146f54489f6ef0f39698396658eda56738cc2d01ed5a4dabc97` |
| 1M compiled | 139,964,416 | 260,307,974 | `9dd6df2483cdebdcae27fabea29b6e13771a0e2ded4924dc7d72427090474705` |
| 10K reasoned | 1,867,776 | 3,073,976 | `6cbd4f867ea87ef468553a536e535b13521046f4c520c400f6966d9d7d09f40c` |
| 100K reasoned | 18,309,120 | 30,337,824 | `a6a1350ab866212248c01955e2dbf6de554812992f5c2fe8abae612dbb646fd5` |

pack 的 10K / 100K 字节同为 **394,534 / 3,886,533**，前后 pack SHA 亦一致；各组 manifest/pack 的完整摘要见对应 raw oracle。100K 三个复杂 focused 请求的 scratch 从约 **12 MiB 降至约 0.004 MiB**，但不包含已挂载的私有数据库，不等于零存储。

## 7. 初版候选与只读分页诊断

初版 [`83e78548d76c11c1e8eaf443de12dbcf91a20495`](https://github.com/Codefor/ontokiln/commit/83e78548d76c11c1e8eaf443de12dbcf91a20495)的 [candidate-focused-session.json](shacl-query-sqlite-20261113/candidate-focused-session.json)保留 **72 条记录：68 COMPLETE、1 次实际失败预热、3 SKIPPED**。该文件完整保留，未拼入正式 after。

| 候选用例 | 10K ms | 100K ms | 1M ms / RSS MiB |
|---|---:|---:|---:|
| nonlocal | 11.502 | 32.074 | 1309.144 / 23.734 |
| 独立 path | 11.336 | 51.962 | 2598.062 / 23.777 |
| 全扫描聚合 | 42.128 | 216.880 | 预热 60.038 s 后预算失败；31.648 MiB |

候选 1M nonlocal/path 均正确完成，语义摘要保持一致，但背景 100K → 1M 时中位数增长 **40.82 / 50.00 倍**。全扫描在 API 执行约 60.036 s 时终止，scratch 10.101 MiB、外层 watchdog=false，观测指向请求时间预算耗尽。低 RSS 证明它避免了旧全图装载的内存失败，仍有严重的规模相关延迟；仅凭 raw 不能确定 SQL 原因。

随后取得的[planner-evidence.json](shacl-query-sqlite-20261113/planner-evidence.json)使用实际构建的 SQLite **3.54.1** 静态库及匹配 header，保留 SQL、bindings、EXPLAIN QUERY PLAN、VM steps 和 probe/library/header SHA。附带 [planner-probe.c](shacl-query-sqlite-20261113/planner-probe.c)、[skew-fixture.sql](shacl-query-sqlite-20261113/skew-fixture.sql)，两份数据库诊断前后 SHA 未变化。

| 1M 数据、256 行页面 | 初版 SQL VM steps | 修正 SQL VM steps | 计划观测 |
|---|---:|---:|---|
| 首页 | 31,002,686 | 7,462 | 初版扫描 OSP、999,999 fullscan steps 后排序；修正沿 SPO 读取页面，无排序 |
| 后续页 | 16,202,349 | 8,510 | 初版以 terms 为外层、建立 DISTINCT 临时树；修正按 facts 复合键范围继续读取，无排序 |

诊断覆盖 1M compiled 与偏斜多图 `skew-72002` 夹具（36,001 条去重 union）、8 种 S/P/O 绑定掩码、首/后页，共 **64 条计划记录**；修正 SQL 的 32 条记录最大 **13,130 VM steps**。新回归按全部 8 种掩码要求每页低于 **64,000 steps**，阈值未放宽，已通过。

这些数据确认了具体 SQL 的全量排序/过宽访问问题及修正；VM steps 不是端到端时间，不能拿其比率冒充 SHACL 加速。JSON 的 `candidate` 指修正后的 SQL，区别于上表的初版二进制；最终端到端结论只采用第 5–6 节正式测量。候选和两份 initial before 的 Rust 元数据缺失，比较器拒绝正式可比性，旧文件未被修补。

## 8. 兼容、剩余边界和冷启动

旧兼容快照可能没有 SPO 索引，或索引定义不适用。无 SPO、错序、partial、方向/排序规则不匹配均由[store 回归](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/tests/shacl/sqlite_store.rs)覆盖；回退读取不修改快照，也不保证旧布局有相同性能。`shapes_from_dataset` 仍读取完整数据图作为 Shapes 图；宽松或策略不明的数据保留原有全图词项校验路线。

排序、聚合、连接、去重、路径访问集合及全 targets coverage 仍可能随工作量增长；256 行分页和有限词项缓存不代表整条查询恒定内存。原有 SHACL 结果行数/字节、RSS、时间、scratch 限制继续生效。

原生 `kernel-manifest-v2` 仍要求 kernel 身份相同，不匹配时重建或选择匹配产物。其他现有兼容产物按原规则使用；本轮 v1 产物摘要相同，不新增一律重建要求，也不承诺任意旧产物跨 kernel 无条件可用。[产物准入](https://github.com/Codefor/ontokiln/blob/b8ecf8046a286345926077318a78dde7541fb98b/src/full_artifact.rs)

| compiled 挂载，单次观测 | 前版进程至 READY / 内部挂载 ms | 新版进程至 READY / 内部挂载 ms | 双方私有数据库复制字节 |
|---:|---:|---:|---:|
| 10K | 5.744 / 4.055 | 11.501 / 8.388 | 1,409,024 |
| 100K | 32.951 / 31.140 | 55.467 / 53.652 | 13,369,344 |
| 1M | 334.094 / 330.861 | 448.225 / 443.887 | 139,964,416 |

三个新版挂载观测均更慢，但各只有一个样本，不计算稳定回退率。新实例仍支付核验与一次快照复制，每次请求仍启动 worker。成功读取中的 **48 条 before、60 条 after**（含预热）全部 `snapshot_reused=true`、`snapshot_bytes_copied=0`。这证明复用而非查询结果缓存；以上 tmpfs、未清 OS 缓存的测量不代表真实冷盘启动或生产 SLO。

## 9. 记录总数与验收

正式四份报告共 **452 条计划记录：440 COMPLETE、3 次实际预算失败预热、9 SKIPPED**。before 合计 214 COMPLETE、3 失败、9 跳过；after **226 条全部 COMPLETE**。COMPLETE 包含构建、mount 和预热，不等于 440 次正式查询；所有成功记录 `oracle.checked=true`。失败及不完整 stage 不输出前后倍数。

| 验收项 | 已核验状态 |
|---|---|
| fmt、Clippy、默认 workspace、all-features | 最终同 tree 全部 exit 0；Clippy 将警告视为错误 |
| scenario_lab 独立预期 | 同 tree exit 0，14/14 |
| 原生、store 与差分 | root lib 49：25 store、12 差分、12 其他，全部通过 |
| API / session / SHACL / Full | 35 / 8 / 11 / 15，全部通过；覆盖 typed 参数、循环路径、union/GRAPH、错误传播、预算与回收 |
| harness / comparator Python | 65/65；随后 harness 未修改，本轮扩展对应单文件 36/36 |
| release 正式测量与比较器 | 两套 after 全部通过 oracle，两组比较器可比、0 issues |
| 远端 CI | 由 [PR #1](https://github.com/Codefor/ontokiln/pull/1) 单独核验最终报告提交的运行状态，不以本文本地门禁结果替代 |

构建来源、同 tree 证明、工具链及门禁日志摘要见 [build-receipt.json](shacl-query-sqlite-20261113/build-receipt.json)。本报告记录被测代码 b8；报告作为后续提交发布，远端 CI 链接与真实状态在 PR 中更新。

## 10. 复现与 raw 校验

固定前后源码分别构建 release，双方使用同一冻结 harness；构建不计入查询时间。运行前将 Rust 1.95.0 加入 PATH，确认 `rustc -vV` 可用。保留 512 MiB / 60 s 失败记录，若另测大预算，需双方对称测量并另存结果。

```sh
export PATH="/absolute/path/to/rust-1.95.0/bin:$PATH"
rustc -vV
TMPDIR=/tmp python3 -B benchmarks/kernel_baseline.py \
  --binary /absolute/path/to/before-or-after-ontokiln --output /absolute/path/to/focused.json \
  --suite focused-shacl --read-transport session --sizes 10000 100000 1000000 \
  --build-samples 3 --query-samples 9 --validation-samples 3 \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 --threads 1 \
  --build-profile release --build-features default --snapshot-guard manifest
TMPDIR=/tmp python3 -B benchmarks/kernel_baseline.py \
  --binary /absolute/path/to/before-or-after-ontokiln --output /absolute/path/to/kernel.json \
  --suite kernel --read-transport file --sizes 10000 100000 \
  --build-samples 3 --query-samples 3 --validation-samples 3 \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 --threads 1 \
  --build-profile release --build-features default --snapshot-guard manifest
```

使用 `benchmarks/compare_kernel_baseline.py BEFORE AFTER --format markdown --output COMPARISON.md`，分别比较同 suite/transport 的正式文件。诊断附件为 [initial-before-focused-session.json](shacl-query-sqlite-20261113/initial-before-focused-session.json)、[initial-before-kernel-file.json](shacl-query-sqlite-20261113/initial-before-kernel-file.json)及候选，均不混入正式四报告。附件从对应 `round3-*` 原始文件原样复制，SHA-256 如下。

| raw 角色 | SHA-256 |
|---|---|
| 正式 before focused | `2ecbdaf112c70b21b868313e97cd52d3c41da514f459df98d6708f1385a994b6` |
| 正式 after focused | `27f9219b079b4eb35b5731dbc0865e6b25bc769a0ac4100e799c6a211d4dcb8b` |
| 正式 before kernel | `f0ae8f00c8eb150eae2d5a9eb3656460160107cc49e9fd909e5576ac6ae1b1eb` |
| 正式 after kernel | `e97a7244367ef9a54bb9cf82926d68251967763f26a170401fd313b6a4067c9d` |
| initial before focused | `b276aaae3e851fd7eb169917a3fffe9b5dafb5467923fef25a54e82369488053` |
| initial before kernel | `48931317d040d658115d8ad94f163fd16c4b59242c49e24d2b5a4bfbc9ccd060` |
| 初版候选 focused | `7870b48c2b68f0c42bf0d77f572d5a320a905501f385284cc0a763fd82871bbe` |
| planner evidence | `67ece580d003cd61023fcbb89042719c9228f694ee0cd6dfabdeaa3bb23206cb` |

本轮合成负载不代表完整 SHACL、生产图谱、macOS、HTTP 吞吐、多租户并发或 Full 亿级容量。完成范围以完整样本和实际预算终态为准；选择性访问、真正全扫描和一次性挂载应分别解释。
