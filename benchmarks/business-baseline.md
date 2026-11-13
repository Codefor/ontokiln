# 真实业务库基线：一条命令运行

使用已经构建好的 OntoKiln 二进制和业务方现有的 [API v1 请求](../docs/api-v1.md)，测量真实 RDF 的编译、规则物化，以及选定的 SPARQL / SHACL 请求。脚本保留输入语义，记录执行结果与资源成本，业务答案由调用方按独立预期核对。

## 运行

在仓库根目录运行这一条命令，将路径换成自己的文件和磁盘目录：

```sh
python3 -B benchmarks/business_baseline.py \
  --binary /path/ontokiln \
  --compile-request /path/real-compile.request.json \
  --read-requests /path/read-cases \
  --work-dir /disk/bench
```

只有 `--compile-request` 必填。`--binary` 默认使用仓库的 `target/release/ontokiln`；工具使用现有二进制，不执行构建。`--read-requests` 可省略，此时报告明确只覆盖编译和规则物化；若提供该参数，目录必须存在且包含 JSON 模板。`--work-dir` 默认是 `data/business-baseline`，建议选择有足够空间的磁盘目录。运行环境为 Linux 或 macOS，脚本只依赖 Python 标准库。

`--compile-request` 是完整的 Full `compile` 请求 JSON，必须显式提供非空 `sources` 和实际使用的 `profile`。`--read-requests` 递归收集目录中的 JSON 文件；每个文件是一条完整的 Full API v1 `query` 或 `validate` 请求模板，主 `query` 支持 SELECT / ASK。查询文本、参数和 Shapes 选择都留在请求里，无需另写一套基准用例格式。

`sources`、`rules`、`shapes` 的每个文件项都必须包含实际输入的 `expected_sha256`（64 位小写十六进制），并指向普通文件，不能是符号链接。工具先核对文件摘要，不自动补写或刷新它们。

输入文件引用中的相对路径默认相对于**启动命令时的工作目录**解析，而不是相对于 JSON 所在目录；`--input-cwd` 可以指定另一输入基目录。

## 本轮实际执行什么

默认串行执行三次独立编译，再从首个成功编译产物出发，独立物化三次；物化不会以上一轮的推导产物继续累加。每条读取请求先预热一次，再测三次。`--build-samples`、`--read-samples`、`--warmups` 可调整这些次数；`--threads` 默认为 1。

读取请求统一指向本轮选定的产物。`--read-dataset reasoned` 是默认值，选择首个成功物化产物；需要编译态对照时使用 `--read-dataset compiled`。这个选择作用于本轮全部读取请求，模板里的 `view` 保持原样；省略时仍按 API 的 `asserted` 默认值执行，选用 reasoned 产物不会自动改成 `closure`。

运行器分配新的产物路径、设置本轮读取所需的数据集引用与摘要，保留 sources、document_id、base_iri、profile、rules、import_lock、查询文本、参数、VALUES、derived_queries 和 Shapes/focus 选择。输入已有的摘要约束照常核验，错配时明确失败，不刷新摘要或修改锁文件。

`.rq` 被编译接纳不表示查询已经执行，也不表示其结果已持久物化。`derived_queries` 可包含 CONSTRUCT，随每次读取请求执行，产生请求内派生数据，其成本计入该请求；规则 `materialize` 的测量单独记录。

每次请求使用文件 API v1 和独立进程，包含该边界下的快照验证、复制与执行成本。本工具没有常驻 session 测量；与其他报告比较时，应保持二进制以外的输入、预算、计时边界和机器条件一致。

## 预算和重复次数

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

执行期间可用 Ctrl+C（SIGINT）或 SIGTERM 受控中断，退出码分别为 130 / 143。运行器终止当前 API 命令进程组、等待直属 supervisor 进程结束，并保存中断状态、已有样本和结果包；未取得 API 回执时，不据此声明 `worker_reaped=true`。

## 去哪里看结果

每次运行在 `--work-dir` 下创建新的子目录，子进程 `TMPDIR` 也放在其中。原始模板保存在 `inputs/`，实际请求、响应和日志保存在 `requests/`，便于核对业务答案；`artifacts/` 默认保留首个成功编译/物化产物，其余重复产物在记录后清理。`--keep-all-artifacts` 可保留全部产物。命令末尾打印结果包路径。

阶段名为 `compile`、`materialize` 和 `read/<模板相对路径.json>`，可直接对应到输入用例。

| 文件 | 内容 |
|---|---|
| `report.md` | 阶段状态、成功样本数、耗时，以及成功正式样本中最高 RSS / scratch（MiB） |
| `summary.json` | 全部样本、状态、阶段资源指标、事实计数、产物统计与本地详细诊断 |
| `environment.json` | 机器、文件系统、运行及构建环境、二进制 SHA-256 |
| `identity.json` | 引擎的 kernel identity |
| `results.zip` | 分享版 `summary.json` / `report.md`，以及环境、身份文件；查询文件、原始请求/响应、RDF 和 SQLite 文件不入包 |

资源列只汇总成功正式样本；失败样本已经采集的资源指标仍保存在 JSON 中。需要分享测量记录时，使用 `results.zip`：其中的分享版摘要按白名单保留状态、计数、指标、摘要和稳定错误 code/reason，去除错误全文、stderr 与协议诊断文本；详细诊断仍在本地 `summary.json` 和 `requests/`。

结果包仍保留路径、用例名、源文件摘要及构建环境，并非匿名化数据。脚本不上传文件。输入预检失败时也保留失败摘要和报告，环境及身份文件以实际收集到的内容为准。

## 如何解释“完成”

报告将**协议执行完成**与**业务正确性**分开，业务正确性标记为 `MANUAL_REVIEW_REQUIRED`。调用方需要按已有独立预期核对查询结果、推导范围，以及 Shapes/focus 覆盖和违规项；工具不会把本次引擎输出自动当成正确答案。

有效的 `conforms=false` 是一次完成的 SHACL 执行，表示发现不符合约束的数据；是否符合业务预期仍需核对。Shapes 或 focus 覆盖为零表示本次校验范围为空，不能据此确认业务约束已经通过。没有提供读取请求时，SPARQL 与 SHACL 属于本轮未测范围。
