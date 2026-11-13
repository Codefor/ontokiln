# 真实业务库基线：一条命令运行

直接指定业务本体仓库目录，工具发现 RDF、规则、SPARQL 和 Shapes，生成 [API v1 请求](../docs/api-v1.md)并运行编译、规则物化与读取基线。**不需要先准备请求 JSON。** 实际输入、生成的请求、未测范围和执行记录都会保留，业务答案仍由调用方核对。

## 运行

在 OntoKiln 仓库根目录运行这一条命令，将路径换成自己的本体仓库：

```sh
python3 -B benchmarks/business_baseline.py --ontology-repo /path/to/business-ontology
```

从其他目录运行时，把脚本路径换成绝对路径即可。`--work-dir /disk/bench` 可指定有足够空间的待测磁盘，默认是 `data/business-baseline`。运行环境为 Linux 或 macOS，脚本只依赖 Python 标准库。

默认 `--jobs auto` 根据可用 CPU、内存和磁盘预算安排多个独立读取用例并发执行，预算不足时可以降为串行。需要单请求延迟基线时，加 `--jobs 1`；主命令和输入方式不变。

并发选项和脚本更新只对新启动的进程生效，已经运行的任务保持原逻辑。本工具没有续跑功能；重新执行会创建独立结果目录，不合并旧运行或中断运行的样本。

仓库模式未指定 `--binary` 时，先找 OntoKiln 的 `target/release/ontokiln`，再找 PATH 中的 `ontokiln`；都没有且可用 Cargo 时，自动进行 release 构建。构建在测量之外。已有二进制按其实际身份使用，不自动升级；`--binary` 可固定待测版本。没有可用二进制或 Rust 构建环境时，工具明确报错并保留已生成文件。`--prepare-only` 只发现资产、生成请求，不要求二进制，也不进行性能测量。

## 自动发现的范围

扫描整个指定目录中的 `.ttl`、`.nt`、`.trig`、`.nq`，**包括 tests、examples 及并存版本**，不猜哪个版本属于生产。跳过 `.git`、`.hg`、`.svn`、`.venv`、`venv`、`node_modules`、`__pycache__`、`target`、本次 work-dir、生成目录和符号链接；可重复添加 `--exclude` glob，例如 `--exclude 'tests/**'`，显式缩小范围。glob 按各扫描根的相对 POSIX 路径匹配。

| 资产 | 自动处理 |
|---|---|
| RDF 文件 | 计算实际字节 SHA-256，以仓库相对路径作为 document ID；不猜测 `base_iri` |
| JSON 规则 | 仅接纳原生 `ontology-rule-ir/v1`，普通业务 JSON 不转成规则 |
| `.rq` / `.sparql` 的 SELECT、ASK | 保留查询原文，生成读取用例；识别到参数槽、模板占位符、`$this` 或空 VALUES 时列为待补充 |
| 无参 CONSTRUCT | 每个文件独立作为 `derived_queries`，配合 `ASK {}` 测量构造过程；不猜跨文件执行顺序 |
| SHACL | 默认从数据集中的 Shapes 生成校验；`--shapes-root` 可明确指定独立的外部 Shapes 根目录 |

仓库模式默认采用 `--profile rdfs-property-chain-v1`；读取 reasoned 产物时默认 `--query-view closure`，选择 compiled 产物时默认 asserted，显式指定的 `--query-view` 保持不变。这是**固定、公开的基准策略**，不是自动识别出的生产配置。默认 Shapes/focus 按引擎合同发现，不把所有 subject 当作 target。`owl:imports` 只解析本地输入，缺失导入按实际引擎行为失败，不自动联网补齐。

`--shapes-root` 在仓库之外时，其 RDF 只用于外部校验，不自动加入编译输入；在仓库之内时，其中 RDF 仍按仓库扫描规则进入编译。未支持的 RDF 格式、压缩 RDF、未接纳的规则版本和不可独立执行的查询会列出原因码；语法和能力仍以引擎实际执行结果为准。

`.rq` 准入不表示查询已经执行或持久物化。CONSTRUCT 的派生数据只对该次请求可见，成本计入该请求；`ASK {}` 探针不能证明构造结果符合业务预期。缺参数等待补充资产会列在清单里，不自动编造业务参数或工作流。

## 本轮实际执行什么

编译和物化保持串行：默认三次独立编译，再从首个成功编译产物出发，独立物化三次；物化不会以上一轮的推导产物继续累加。读取阶段可以同时运行不同用例，但**同一用例仍顺序预热一次、正式测三次**。`--build-samples`、`--read-samples`、`--warmups` 可调整这些次数。

`--jobs` 控制独立读取用例的并发。`--threads` 默认为 1，只设置引擎的 Rayon 线程数，不会把 Full 的单个查询自动变为多核查询；只有一个读取用例时，提高 jobs 也不会拆分它。提高 jobs 主要考察一批用例的完成时间与吞吐；多个请求也可能争用 CPU、缓存和磁盘，单次延迟需要分别观察。

读取请求统一指向本轮选定的产物。`--read-dataset reasoned` 是默认值，选择首个成功物化产物；`--read-dataset compiled` 选择编译态。这个选择作用于本轮全部读取请求，不重写请求中的 `view`；视图必须与所选快照的能力匹配。

运行器分配新的产物路径、设置本轮读取所需的数据集引用与摘要，按生成或提供的请求执行规则和查询。CONSTRUCT 探针与规则物化分别计量。

每次请求使用文件 API v1 和独立进程，包含该边界下的快照验证、复制与执行成本。本工具没有常驻 session 测量。比较内核版本时，应保持输入、每请求预算、实际准入 jobs、总准入预算、threads、计时边界和机器条件一致；并发批次总耗时的缩短不能当作单查询优化倍数。两次都使用 auto 不保证实际并发相同，应核对报告中的 `effective_jobs`。

## 预算和重复次数

`--total-memory-mib` 和 `--total-scratch-mib` 设置读取并发的**准入上限**。编译/物化阶段结束后，运行器探测 CPU、可用内存和工作目录剩余磁盘；每个并发槽位按所有读取请求中最大的 memory/scratch 上限计算。可探测的内存和磁盘容量先留出 20%，再与显式总上限取较小值，并受 CPU、threads、用例数和请求 jobs 限制。容量未知时并发最多为 1；显式 `--jobs N` 也受这些限制。

这是规划时的容量快照，不修改每请求 API 预算，不是操作系统硬预留或隔离；其他进程仍可能改变可用容量。已知准入容量连一个上述槽位也容纳不下时，读取全部标为 SKIPPED，整体状态为 `READ_ADMISSION_FAILED`、退出码 2，不能当作串行成功。缺少所选 compiled/reasoned 产物则记录读取 `SKIPPED_DEPENDENCY`，不启动读取请求。

未提供的预算字段使用下表默认值；请求模板中的预算覆盖默认值，显式 CLI 参数再覆盖对应字段。实际派发请求和最终预算保存在结果目录中。

自动运行要求 `memory_bytes`、`scratch_bytes`、`timeout_ms` 都是有限正数；内存和 scratch 还必须是整数。模板明确传入 null、非有限数或非正数时，预检会拒绝，不会用缺省值代替。需要为原本无界的模板指定本轮上限时，使用对应 CLI 参数显式覆盖。合法的零行数、零新增事实数或零迭代次数保持为零。

| 预算 | 缺省值 | 显式覆盖参数 |
|---|---:|---|
| combined RSS | 4 GiB | `--memory-mib` |
| scratch | 8 GiB | `--scratch-mib` |
| 单请求时间 | 300 s | `--timeout-seconds` |
| 物化迭代次数 | 100 | `--max-iterations` |
| 新增事实数 | 10,000,000 | `--max-derived-facts` |
| 中间行数 | 100,000,000 | `--max-intermediate-rows` |

其余未给定的限制为 `max_rows=100000`、`max_bytes=4194304`（4 MiB）、`max_validation_depth=64`、`max_term_bytes=1048576`（1 MiB）；这些字段可在请求模板的 `limits` 中指定。

失败、预算耗尽和跳过分别保留，不进入成功延迟中位数，也不能用于计算加速比。预热与正式样本分开记录；默认三次样本用于观察中位数和范围，不报告尾延迟保证。

执行期间可用 Ctrl+C（SIGINT）或 SIGTERM 受控中断，退出码分别为 130 / 143。运行器终止当前构建或 API 命令进程组、等待直属命令进程结束，并保存中断状态、已有样本和结果包；未取得 API 回执时，不据此声明 `worker_reaped=true`。

## 去哪里看结果

每次运行在 `--work-dir` 下创建新的子目录，子进程 `TMPDIR` 也放在其中。仓库模式生成 `generated/compile.request.json`、`generated/read-cases/` 和 `generated/inventory.json`；原始模板副本保存在 `inputs/`，实际请求、响应和日志保存在 `requests/`。`artifacts/` 默认保留首个成功编译/物化产物，其余重复产物在记录后清理；`--keep-all-artifacts` 可保留全部产物。命令末尾打印结果包路径。

阶段名为 `compile`、`materialize` 和 `read/<模板相对路径.json>`；自动生成的读取用例通过 inventory 对应到原始资产。

日志显示 START/终态、用例与样本序号、单次耗时和总已用时间。`resolved/total` 包括成功、失败与跳过，`active` 是当前活动请求数；读取阶段连续约 15 秒没有用例结束时输出运行心跳，同一用例内部仍可正常输出样本进度。

`summary.json` 的 `parallelism` 记录 requested/effective jobs、每槽位预算、资源观测和限制原因。`read_execution` 区分 `serial_latency` 与 `concurrent_workload`，记录 `elapsed_seconds`、请求计数和 `peak_active_requests`；峰值是运行器记录的活动读取请求数，不是总物理 RSS。`progress` 分列成功、失败、跳过和活动请求。

读取阶段的平均完成率 `completed_requests_per_second` 以完成请求数（含成功预热）除以整个读取阶段墙钟时间；分母包含预热、调度、检查和失败处理。它不是仅正式样本的吞吐，也不是单一查询的独立 QPS。逐请求中位数仍只使用成功正式样本。

| 文件 | 内容 |
|---|---|
| `generated/inventory.json` | 自动发现的资产、生成的用例及待补充范围 |
| `report.md` | 阶段状态、成功样本数、耗时，以及成功正式样本中最高 RSS / scratch（MiB） |
| `summary.json` | 全部样本、状态、阶段资源指标、读取并发与总耗时、事实计数、产物统计及本地详细诊断 |
| `environment.json` | 机器、文件系统、运行及构建环境、二进制 SHA-256 |
| `identity.json` | 引擎的 kernel identity |
| `results.zip` | 分享版 `summary.json` / `report.md`，以及环境、身份文件；查询文件、原始请求/响应、RDF 和 SQLite 文件不入包 |

资源列只汇总成功正式样本；失败样本已经采集的资源指标仍保存在 JSON 中。需要分享测量记录时，使用 `results.zip`：其中的分享版摘要按白名单保留状态、计数、指标、摘要和稳定错误 code/reason，去除错误全文、stderr 与协议诊断文本；详细诊断仍在本地 `summary.json` 和 `requests/`。

结果包仍保留路径、用例名、源文件摘要及构建环境，并非匿名化数据。脚本不上传文件。输入预检失败时也保留失败摘要和报告，环境及身份文件以实际收集到的内容为准。

## 如何解释“完成”

报告将**协议执行完成**与**业务正确性**分开，业务正确性标记为 `MANUAL_REVIEW_REQUIRED`。调用方需要按已有独立预期核对查询结果、推导范围，以及 Shapes/focus 覆盖和违规项；工具不会把本次引擎输出自动当成正确答案。

`--prepare-only` 的状态为 `PREPARED`，没有测量结果。仓库模式下，执行全部成功但仍有未测资产或空校验范围时，状态为 `COMPLETE_WITH_GAPS`、退出码为 3；报告会列出缺口。即使状态为 `COMPLETE`，业务正确性仍需人工复核。

有效的 `conforms=false` 是一次完成的 SHACL 执行，表示发现不符合约束的数据。Shapes 或 focus 覆盖为零表示本次范围为空，不能据此确认业务约束已经通过。

## 已有精确请求时

可使用 `--compile-request` 代替 `--ontology-repo`，并以可选的 `--read-requests` 指向读取请求 JSON 目录。该模式沿用已有请求，不自动选 profile、猜参数或补工作流；未提供读取目录时只测 compile/materialize。主 `query` 为 SELECT / ASK，`derived_queries` 可含 CONSTRUCT；原有 `view` 保持不变，省略时仍为 API 默认的 `asserted`。

完整 compile 请求必须显式提供非空 `sources` 和 `profile`，sources/rules/shapes 文件项须带实际 `expected_sha256` 并指向普通非符号链接文件。已有摘要和 import_lock 照常核验，不刷新或改写。输入引用中的相对路径按启动目录解析，`--input-cwd` 可指定输入基目录。此模式使用现有二进制，不自动构建；通过 `--binary` 可指定路径。

## 仓库里的性能回归

真实与合成本体目录工作负载的集成性能回归统一使用本入口。需要隔离单请求变化时固定 `--jobs 1`；需要观察一批请求的并发完成情况时固定相同的 jobs、总准入预算和每请求预算。业务答案仍由独立预期核对，不能仅凭执行完成判定正确。

单元测试、API/Full 契约回归及其冻结输入继续验证行为；固定生成器的 1M/10M 等规模基准继续使用原方法和独立 oracle。它们与本入口各有职责，不用真实性能运行替代全部测试，详见[基准与测试说明](../tests/benchmarks/README.md)。

CI 的[原生小图 smoke](../tests/benchmarks/business_native_smoke.py)也只调用本入口一次：用[静态合成库](../tests/fixtures/business-baseline/README.md)核对 9 条显式事实、2 条推导事实、11 条闭包事实、有序 RDF 绑定、ASK 和两个精确 SHACL targets。预期答案写在测试中，原始响应与独立核验记录保留；这是功能验证，不作为性能成绩。
