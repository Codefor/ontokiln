# 消费者契约 v1

`ontokiln-api/v1` 是面向消费者的稳定 JSON 文件协议。实现入口为 [api.rs](../src/api.rs) 的 `Request` / `Operation` 和 `run_files`，CLI 为：

```sh
ontokiln api --request request.json --result result.json
```

生产服务需要反复读取同一 Full Bundle 时，可使用 [常驻实例适配器](session-v1.md)
`ontokiln api-session --dataset ...`。它在启动时核验并挂载一次快照，随后接收相同
API v1 请求、返回相同结果语义。业务层继续使用统一的 query/validate 接口，只在
调用适配层持有引擎实例。单次命令也使用同一新版 FullSnapshot 内核，只是实例随请求结束而释放；
逐次启动进程不会自动获得跨请求复用，不需要维护两套引擎实现。
当前复用数据库快照与可用的操作系统页缓存，查询计划、SQLite 连接及 SHACL 求值状态仍按请求创建；满足严格解析条件的 Full 校验直接读取 SQLite，不预先复制完整数据图到内存。

同一接口主版本、同一语义快照、同一规则和请求条件下，内核优化不要求消费者修改调用代码，完整成功结果保持语义等价。构建 SHA、执行时间、内存与内部执行计划不属于结果等价比较；显式选择的读取格式由独立版本契约约束；运行资源变化可能使同一预算下的完成情况不同。超限、取消或能力缺失必须明确失败，不返回部分成功。

## 边界与兼容

```mermaid
flowchart TB
    C[消费者] --> A[ontokiln-api/v1：请求、结果、错误与语义]
    A --> S[独立 worker：预算、取消、回收]
    S --> P[pack 适配：索引与层级推理]
    S --> F[Full 适配：编译、物化、查询与 SHACL]
    T[固定输入与独立预期回归] -.约束.-> A
```

- `api` 不接受后端选择开关。编译返回不透明的 `dataset` 路径，后续操作使用该路径；常规查询消费者只传回该路径。需要本地批量读取的消费者可依赖下文独立的 `ontokiln-sqlite-read/v1`，不能依赖未声明的内部表或跨快照词项 ID。适配器目前识别 Full 产物目录和已有 pack 文件。
- 既有 `full`、`compile`、`sparql`、HTTP `/v1/*` 与 Rust 模块继续提供原有行为。新入口默认 `asserted`；旧 Full 的默认 `closure` 不被修改。应用迁移应同时切换调用、启动能力检查、终态验证及发行 pin；保留旧入口不代表迁移后的应用继续调用旧协议。
- `src/full*`、未声明的磁盘表、pack 字节和构建身份属于内部协议或特定存储格式。`identity check` 仍用于精确版本核验，不作为 API 向后兼容判断。API 主版本与存储版本分别演进；旧物理文件的重编策略不等于 API 升级。
- `run_files` 使用一次调用一个独立 worker 的执行方式，由当前可执行程序启动并在完成后回收；请求或启动检查失败时可能不启动 worker。进程、PID 和回收回执由 v1 执行适配层负责；更换内部执行器不能把“任务完成”冒充“进程已回收”。Rust 嵌入者须保留 CLI worker 分派，或调用发布的 `ontokiln api` 进程；直接调用旧 `full::execute` 不经过此契约及进程隔离。
- 本版覆盖本地 SELECT / ASK、编译、物化、SHACL 和能力查询。HTTP 模板、JSONL、证明导出等现有入口继续按原协议工作；本版不承诺逐事实证明。`require` 请求不存在的能力会明确失败。

版本分别约束不同内容：API 版本约束请求、结果、错误和默认行为；数据集语义标识约束图与视图的含义；规则 profile 约束规则集合和推导语义；`read_contract` 约束显式公开的读取格式。替换算法或索引应保持这些语义，扩展已有 profile 的推导范围须使用新的 profile。构建身份用于发行核验，不替代上述兼容判断。

`ontokiln-api/v1` 的稳定边界是 JSON 文件协议。公开 Rust 模块、struct / enum 的源码接口及 ABI 仍是实现接口；嵌入式 Rust 接口需要独立定义兼容契约。

v1 保持既有默认 envelope、查询和校验结果的字段集，不要求现有消费者事后改为忽略新增字段。新增可选请求字段必须保持省略时的原行为；新增结果元数据须由显式选择、新操作或新版本承载。`operations`、`capabilities`、`read_contracts` 等能力列表允许增加条目，消费者按集合包含关系判断所需能力，不依赖顺序或完整列表相等。

## 请求

字段名、类型及未知字段拒绝由 Rust 的 Serde 类型执行，见 [Request / Operation / Limits](../src/api.rs)。禁止将任意内部 JSON 透传为公开请求。

```json
{
  "contract": "ontokiln-api/v1",
  "request_id": "lookup-1",
  "operation": {
    "kind": "query",
    "dataset": "/data/snapshot",
    "query": "SELECT ?s WHERE { ?s a <urn:example:Item> }",
    "view": "asserted",
    "require": ["select-ask/v1"]
  },
  "limits": {"timeout_ms": 30000, "max_rows": 100000}
}
```

`contract` 和 `request_id` 必填；后者为 1～256 UTF-8 字节。先检查 JSON 对象及字符串类型的 `contract`，再按对应版本解析请求：未知版本返回 `UNSUPPORTED_CONTRACT`，即使其 operation 不属于 v1；缺失或类型错误的 `contract` 返回 `INVALID_REQUEST`。对 v1 请求，未知字段、错误类型、非法枚举和无效预算返回 `INVALID_REQUEST`。路径按进程工作目录解析，推荐传绝对路径。请求文件没有独立的固定字节上限，读取与解析的资源边界见“预算”。

| operation.kind | 字段与行为 |
|---|---|
| `capabilities` | 可选 `dataset`。无数据集时返回操作集合、默认视图和证明覆盖；有数据集时校验后返回其语义、profile、能力及可用视图 |
| `prepare_query` | `query`。检查本地 SELECT / ASK / CONSTRUCT 的语法及本地查询限制，拒绝远端访问；返回 `valid: true`。不绑定数据集、不执行查询，不承诺后续预算内完成 |
| `rules` | `profile`、`files: [{path, expected_sha256}]`。冻结、校验并返回 `ontology-rule-ir/v1` 的 `rules` 数组 |
| `compile` | `sources`、`output`、`profile`，可选 `rules: []`、`import_lock`、`expected_source_tree_digest`。冻结输入并验证 SHA-256，原子发布新快照；输出路径必须不存在 |
| `materialize` | `dataset`、`output`。读取已编译快照的规则计划，求不动点并发布新快照；失败不覆盖原数据 |
| `query` | `dataset`、`query`；可选 `parameters: {}`、`rdf_bindings: {}`、`values: []`、`derived_queries: []`、`view: asserted`、`revision`、`require: []`、`expected_content_digest`、`expected_manifest_sha256`。仅本地 SELECT / ASK，不允许 FROM / SERVICE |
| `validate` | `dataset`；可选 `shapes: []`、`shapes_from_dataset: false`、`selected_shapes`、`focus_node`、`expected_content_digest`、`expected_manifest_sha256`。只能选择外部 Shapes 或数据集内 Shapes；不支持 pack |

`sources` 和外部 `shapes` 的元素为 `{path, document_id, format, expected_sha256, base_iri?}`；`format` 为 `turtle`、`ntriples`、`trig` 或 `nquads`。源文件必须是普通非符号链接文件，摘要为实际字节的 SHA-256。编译时 `document_id` 隔离空白节点；外部 Shapes 的空白节点独立解析。`rules` 元素为 `{path, expected_sha256}`。

编译 profile 必填，可选 `no-rules-v1`、`datalog-v1`、`rdfs-supported-v1`、`rdfs-property-chain-v1`；规则格式与含义见 [Full 合同](full.md)。只解析已冻结的本地本体声明，不联网获取 imports。编译 RDF 词项默认最大 1 MiB，可用 `limits.max_term_bytes` 显式调整；严格解析，不提供隐式业务修复。`import_lock` 为 ontology IRI 到 document ID 或 ID 数组的映射；null 使用冻结声明自动解析，空对象是禁止未声明映射的显式锁。`expected_source_tree_digest` 按冻结来源清单核对 SHA-256，失败不发布产物。

`parameters` 使用显式 `urn:ontokiln:param:NAME` 槽。IRI 为 `{"type":"iri","value":"urn:x"}`；字面量为 `{"type":"literal","value":"文本","language":"zh"}` 或带 `datatype` 的形式。`language` 与 `datatype` 不能同时提供。多余、缺失和非法参数被拒绝；SPARQL 变量仍有自身作用域。

`rdf_bindings` 是独立的变量初始绑定机制，值为合法 N-Triples RDF 词项字符串，变量名不含 `?`。它保持 SELECT 列、SELECT / ASK 中 FILTER/BOUND 和 ORDER BY 的变量语义，允许同一快照的空白节点；不通过文本替换查询实现。只有声明 `rdf-bindings/v1` 的数据集支持，pack 暂时明确拒绝。

`expected_content_digest` 为 query / validate 的可选产物文件前置条件，取自 compile / materialize 成功结果的 `artifact.content_digest`。它绑定文件清单及其字节摘要，**不包含 manifest.json 本身**，因此不单独绑定其中的 kind、profile 或 fixpoint 声明。Full 通过 `dataset-content-digest/v1` 声明此能力，既有摘要公式保持不变。

需要固定完整快照状态时，使用 query / validate 的可选 `expected_manifest_sha256`，取自 compile / materialize 成功结果的 `artifact.manifest_sha256`。它绑定本次完整性核验实际读取并解析的同一份 manifest 原始字节，包括 kind、profile、fixpoint 和文件清单；JSON 排版变化也会改变摘要。Full 通过 `dataset-manifest-digest/v1` 声明此能力。摘要一致证明读取的是指定声明及其经核验的文件，不独立证明规则执行正确，也不是跨重编的语义等价检查。

上述两个字段均可单独提供，也可同时提供；各自省略或 null 不增加对应前置条件。非 null 值必须为 64 个小写十六进制字符，否则返回 `INVALID_REQUEST`。Full 在完成产物完整性核验后比较摘要，任一错配返回 `INVALID_DATASET`，不会返回查询或校验结果；同时核对 manifest 中的 profile、来源摘要和语义指纹与受文件摘要保护的冻结输入是否一致。pack 查询提供任一非 null 摘要时返回 `UNSUPPORTED_CAPABILITY`，不会忽略。两项前置条件均不增加默认响应字段。

## 快照语义

| 快照 | 语义与可用能力 |
|---|---|
| 既有 pack | `pack-rdfs-hierarchy/v1`；默认图为所选文档的去重并集，asserted 命名图是文档图；层级推理与版本隔离维持原语义。能力含 `document-graphs/v1`、`revision/v1` |
| Full | `rdf-dataset-union/v1`；存储保留 RDF 图名，SPARQL 默认图为所有可见图的去重并集，GRAPH 使用原始 RDF 图名；规则以冻结 profile / plan 为准。能力含 `rdf-named-graphs/v1`、`shacl/v1`、`dataset-content-digest/v1`、`dataset-manifest-digest/v1`；不支持 pack 的 revision 投影 |

两者共同提供 `select-ask/v1`、`views/v1`。不同图语义、不同规则 profile 不能仅因接口相同就互换。未来替换存储适配器时必须保留原快照的语义和能力，不能将 Full 具名图自动解释为 pack 文档图。

`asserted` 为原始事实；`inferred` 为新增推导事实；`closure` 为两者并集。Full 先按四元组的 explicit 标记筛选，再投影默认图；同一 SPO 在不同图的显式 / 推导状态可以不同。pack 按所选版本的三元组区分，显式声明优先。Full 带规则的 compiled 快照只允许 asserted，完成物化后才允许 inferred / closure；`no-rules-v1` 的 compiled 快照可直接查询三个视图。reasoned 快照必须记录已完成不动点。pack 的层级视图由受预算约束的查询求值生成。

Full 规则的图范围与查询默认图分别定义：规则 IR 的三元组 atom 省略图位置时固定为物理默认图 `g=0`；四元组 atom 可显式指定图或图变量。当前内置 RDFS、inverse / symmetric 和自动生成的 property-chain 规则均作用于物理默认图；property-chain 声明及列表也要求在该图。SPARQL 默认图的 union 不会自动扩大这些规则的输入范围。更改规则的图范围须使用新的 profile，不能作为索引或执行计划优化悄然引入。

compile 成功表示冻结输入已编译为新快照，不表示已经执行规则物化。materialize 读取 compiled 快照及其冻结规则计划，求不动点后发布新的 reasoned 快照，原快照不变；超限或未达到不动点均不能发布成功。query 只读取指定快照和可用视图，不隐式执行 Full 物化。

validate 校验快照中**已存储全部事实的 SPO 去重并集**：不按 explicit 标记筛选，图名不参与校验数据图；不会在校验前自动执行 profile 规则物化，也不采用 query 的默认 asserted 视图。因此 compiled 和 reasoned 快照可能得到不同校验结果。外部 Shapes 同样合并为去重图；`shapes_from_dataset=true` 使用该数据图中的 Shapes。支持的约束及限制以 [Full 合同](full.md) 为准，`shacl/v1` 不表示支持全部 SHACL 扩展。

Shapes 省略且 `shapes_from_dataset=false` 时为空图；`selected_shapes` 省略时选择声明为 NodeShape / PropertyShape 的节点，显式空数组表示不选择任何 Shape。`checked_shapes` 是所选 Shape 的数量，不是焦点节点或约束求值次数。没有待校验 Shape 时可以 `COMPLETE`、`conforms=true`、`checked_shapes=0`；完成和符合约束均不证明校验范围非空。调用方应按自身要求核对选择范围与报告。

Full 查询与校验先校验 manifest / 文件摘要，再按已验证摘要复制数据库到本次 worker 的私有临时目录。pack 复用加载器的私有只读快照。外部 Shapes 也先核验摘要再冻结。复制和求值都处于执行预算内。

## 响应与完整性

所有正常写出结果文件的请求都使用同一 envelope：

```json
{
  "contract": "ontokiln-api/v1",
  "request_id": "lookup-1",
  "request_sha256": "<原始请求文件摘要>",
  "status": "COMPLETE",
  "result": {},
  "error": null,
  "execution": {"worker_reaped": true, "worker_pid": 12345},
  "kernel": {"contract_version": "ontokiln-core/v3"}
}
```

`execution` 还包含 `supervisor_pid`、`integrity_checked` 和实际执行的 `metrics`：`peak_rss_bytes`、`supervisor_peak_rss_bytes`、`peak_scratch_bytes`、`execution_seconds`。未启动 worker 的请求不要求 metrics。产物所有权回执绑定原始公开请求的摘要和 supervisor PID，供宿主在强杀后仅清理本次输出。

`kernel` 实际包含完整构建身份，示例省略其余字段。未启动 worker 时 `worker_pid` 为 null。只有 `status=COMPLETE`、`error=null` 且 `worker_reaped=true` 才能消费成功结果。错误响应的 `result` 必须为 null。无效 JSON 无法提供 ID 时，`request_id` 为 null。结果文件原子写入；不可读请求或结果文件 I/O 失败属于进程失败，调用方还须检查退出码和本次结果文件，不能复用旧响应。

| result 类型 | 固定内容 |
|---|---|
| 查询 | `query_type`、`variables`、`items`、`boolean`、`view`、`complete=true`、`truncated=false`、`proof_coverage`。SELECT 的 boolean 为 null；ASK 的 variables / items 为空 |
| 编译 / 物化 | `dataset`、`stats.explicit_facts`、`stats.inferred_facts`、`fixpoint_reached`、`proof_coverage`、`read_contract` 和 `artifact`（路径、manifest / 内容 / 来源摘要、语义指纹、profile） |
| 校验 | `conforms`、`checked_shapes`、`violations` 数量、结构化 `report`（含 violations）。不符合 Shape 是正常校验结果，不是执行失败；现有 report 的字段及 RDF 词项表示仍属于兼容范围 |
| 能力 | `operations`、`default_view`、`proof_coverage`，以及按请求提供的 `dataset` 元数据 |

查询词项为 SPARQL JSON 的 `uri`、`bnode`、`literal` 对象；保留词法、datatype 和 `xml:lang`。普通字符串统一显式标记 `xsd:string`。不支持的结果词项明确拒绝。未绑定变量不出现在该行中。未使用 DISTINCT / REDUCED 的 SELECT 保留重复解；只有 ORDER BY 承诺指定顺序。空白节点标签可合法重命名，等价比较需保留同一结果内的节点关系，不能按标签字节比较不同快照。

### 查询结果的语义等价

兼容比较遵循 [SPARQL 1.1 的结果修饰符](https://www.w3.org/TR/sparql11-query/#solutionModifiers)与[聚合语义](https://www.w3.org/TR/sparql11-query/#aggregates)，不把一次执行的遍历顺序当作额外承诺：

- 对完整、确定性的 SELECT 结果，按变量绑定的多重集比较，保留重复次数、未绑定状态和 RDF 词项身份；不能用去重集合掩盖重复解变化。确定性 ASK 比较布尔值。
- ORDER BY 的比较键可比较且足以确定顺序时，按有序结果比较；相同排序键之间、或规范未定义比较关系的解之间，不承诺额外顺序。LIMIT / OFFSET 在没有确定全序或截断同值组时，可以选出不同的合法解，不能要求两次切片的行集合必然相等。
- SAMPLE 的取值、GROUP_CONCAT 的拼接顺序、REDUCED 的重复消除，以及 RAND、NOW、UUID / STRUUID、BNODE 等函数产生的值，按各自规范检查允许的结果；不能要求跨执行字节一致，也不能免除规范要求的取值范围、同一请求内的一致性、唯一性或空白节点关系。

性能基线优先使用确定性的完整计数或完整结果多重集；需要稳定切片或逐行比较时，使用足以唯一确定顺序的稳定排序键。不能仅用行数证明任意结果集正确，也不能从待测引擎的输出反写预期；包含上述不确定行为的查询应使用专门的合法性检查，或不纳入确定性结果差分门禁。

本版 `proof_coverage` 固定为 `{"per_fact":"not_recorded"}`，不把结果数量、闭包或成功终态当作逐事实证明。

### 摘要与身份

| 字段 | 含义与边界 |
|---|---|
| `request_sha256` | 原始公开请求文件字节的摘要；JSON 排版变化也会改变。绑定请求内容，不是幂等键，也不单独绑定 dataset 路径所指的内容 |
| `source_tree_digest` | 按 document_id 排序后的冻结来源清单摘要，包含来源格式、base IRI、字节摘要等；不是 RDF 图的语义规范化摘要 |
| `content_digest` | 产物文件清单及其字节摘要的摘要，排除 manifest.json；不单独绑定 manifest 中的状态或 profile。即使语义相同，重编或物理格式变化也可能改变 |
| `manifest_sha256` | manifest 原始文件字节的摘要，绑定其中的 kind、profile、fixpoint 和文件清单；完整快照前置条件使用对应产物的此摘要，JSON 排版变化也会改变 |
| `semantic_fingerprint` | 冻结输入与配置配方的摘要，包含 engine semantic version 和 index profile。不是 RDF 图语义等价判定；compiled / reasoned 可共享此值，不能据此判断物化是否完成 |
| `kernel` | 实际构建与发行身份；用于诊断、固定发行和精确版本核验，不作为 API 或读取格式的兼容版本 |

### 失败与恢复

| error.code | status |
|---|---|
| `INVALID_REQUEST`、`INVALID_DATASET`、`INVALID_QUERY`、`EXECUTION_FAILED` | `FAILED` |
| `UNSUPPORTED_CONTRACT`、`UNSUPPORTED_CAPABILITY` | `UNSUPPORTED` |
| `RESOURCE_EXHAUSTED` | `BUDGET_EXHAUSTED` |
| `CANCELLED` | `CANCELLED` |

客户端按 code 判断，不匹配诊断 message。可选细分 `error.reason` 保留 `SOURCE_INVALID`、`RULE_INVALID`、`UNSAFE_RULE`、`BUILD_ABORTED`、`TTL_PARSE_ERROR`、`UNSUPPORTED_RULE_PROFILE`、`FORMAT_VERSION_UNSUPPORTED`、`MANIFEST_CORRUPT`、`QUERY_FAILED`、`VALIDATION_FAILED`、`RESOURCE_LIMIT_EXCEEDED`、`BUDGET_EXHAUSTED`、`FIXPOINT_LIMIT_EXCEEDED`、`CANCELLED`；未知 reason 必须退回稳定 code。对 supervisor 发 SIGTERM / SIGINT 会终止并 wait worker 后写 CANCELLED；超时、RSS 或临时磁盘预算失败同样回收 worker。系统强杀或 I/O 失败可能没有终态文件，不能解释为成功。

没有本次终态文件也不表示一定没有副作用：进程可能在产物发布后、结果文件写出前终止。`request_id` 和 `request_sha256` 不提供自动幂等，已有 output 也不会被重试覆盖。宿主应结合本次所有权回执和实际输出核对发布状态，再决定恢复或重试，不能将旧结果文件当成本次成功。

## 预算

`limits` 可省略；字段省略时使用下表默认值。`memory_bytes`、`scratch_bytes`、`timeout_ms` 的显式 null 表示调用方取消该项预算，其余字段不接受 null；仍保留取消与进程回收。预算是执行许可，不是性能承诺。文件请求没有额外的固定字节上限；启动 worker 前的 JSON 读取与解析由调用方进程环境承担，不属于 worker RSS 预算。

Full `compile` / `materialize` 仍要求有限的内存与临时磁盘预算：这两个操作的 `memory_bytes` / `scratch_bytes` 应省略以使用默认值，或传入正整数；传 null 返回 `INVALID_REQUEST`。查询和校验等读取操作允许取消这些预算。

| 字段 | 默认值 | 适用范围 |
|---|---|---|
| `memory_bytes` | 512 MiB | 执行适配层：supervisor 与 worker 的 RSS |
| `scratch_bytes` | 4 GiB | 执行适配层：临时目录与 staging |
| `timeout_ms` | 300,000 ms | 执行适配层：执行、冻结输入及完成处理 |
| `max_rows` | 100,000 | query 最终输出；Full 可显式增大，ASK 计一行 |
| `max_bytes` | 4 MiB | query 最终 payload；Full 可显式增大 |
| `max_iterations` | 100 | materialize 的不动点迭代 |
| `max_derived_facts` | 1,000,000 | materialize 推导事实、query 派生数据 |
| `max_intermediate_rows` | 10,000,000 | materialize 中间结果、query 派生查询累计行数 |
| `max_validation_depth` | 64 | validate 的 Shape 嵌套求值 |
| `max_term_bytes` | 1 MiB | compile 解析 RDF 词项 |

操作专用限制不代替通用执行预算，也不表示其他操作会计量同名指标。`memory_bytes`、`scratch_bytes`、`timeout_ms` 的非 null 值及 `max_validation_depth`、`max_term_bytes` 必须为正；其他计数或输出上限可以为零，但实际工作一旦超过上限即失败。pack 的 `max_rows` / `max_bytes` 不能高于其已支持的上限。

`max_bytes` 检查规范化查询 payload `{query_type, variables, items, boolean}` 的紧凑 UTF-8 JSON 字节数，不包含 envelope，也不是请求文件大小限制。内部扫描、序列化和内存保护仍可提前拒绝请求；不会为了保持完成率突破预算。RSS 为采样及进程高水位验收，不承诺任何瞬时值绝不越界。SHACL 的 SQLite 读取、缓存和求值中间状态仍受独立进程预算限制；Shapes 图及历史解析策略的处理见 [Full 合同](full.md#shacl)。超出公开 `max_validation_depth` 返回 `RESOURCE_EXHAUSTED` / `BUDGET_EXHAUSTED`，不作为普通校验执行失败或 `conforms=false`。

## 契约回归与升级规则

[固定语料](../tests/fixtures/contract-v1/README.md) 含恰好 10,000 条显式 RDF 三元组。`schema.ttl` 为 19 条，`data.ttl` 为 9,981 条；两份文档中的同名空白节点互不合并。生成器和 SHA-256 manifest 检查输入漂移，编译器的实际事实数断言检查解析结果。

```sh
python3 -B tests/fixtures/contract-v1/generate.py --check
cargo test --locked --test api
```

[cases.json](../tests/fixtures/contract-v1/cases.json) 保存独立人工预期；[api.py](../tests/api.py) 同时核对预期与 pack / Full 共同子集差分。覆盖默认值、三种视图、JOIN / FILTER、重复解、排序、类型词法、参数、文档空白节点、能力拒绝、SHACL、输入损坏、原子发布、字节预算、超时和真实 worker 取消。旧接口继续由既有 Full / HTTP / 引擎测试验证。

[legacy_requests.json](../tests/fixtures/contract-v1/legacy_requests.json) 固定已有消费者请求，[legacy_client.py](../tests/fixtures/contract-v1/legacy_client.py) 固定旧消费者对默认 envelope、查询和校验结果字段的解析。新增能力须同时通过这套未适配新字段的消费者回归，不能通过同步修改旧消费者来掩盖默认响应变化。快照摘要前置条件、版本协商和深度预算另有新行为回归。

变更内核必须运行同一套消费者测试。不能只改预期以消除失败；有意改变旧语义、默认值、必填字段、错误码或删除能力时，应新增主版本并给出迁移。构建指纹变化不要求 API 升级。10,000 条数据是契约与机制回归，不是亿级性能证明。

## 本地读取产物契约

`ontokiln-sqlite-read/v1` 是显式支持的批量读取格式，与 API 主版本分别演进。当前 Full 编译和物化直接产生该格式，manifest 通过 `read_contract` 声明它；`capabilities.read_contracts` 表示引擎支持。以后内核换存储、索引或推理实现，仍须通过适配器输出相同读取格式，或提供新版本迁移，不能让应用直接跟随内部表变化。

- 文件集合、来源与内容校验沿用 `kernel-manifest-v1`：`facts.sqlite`、`plan.json`、`input_manifest.json` 及冻结输入文件。manifest 给出完整文件大小 / SHA-256、kind、profile、source_tree_digest、semantic_fingerprint、stats、completion；reasoned 还记录 compiled_content_digest 与 fixpoint_reached。只有完整性核验通过的只读产物可消费。
- `terms(id, n, kind, value, datatype, lang)`：`n` 为 N-Triples 词项，kind 为 I / B / L；空 datatype/lang 用空字符串。ID 仅在该产物内有效。
- `facts(g, s, p, o, explicit)`：四元组集合，s/p/o 与非零 g 引用 terms；g=0 为默认图；explicit=1 为断言，0 为新增推导。同一四元组若已有断言，显式标记优先。
- `source_documents(id, document_id)` 与 `source_facts(g, s, p, o, document)` 保留显式事实的全部来源，document 引用 source_documents。同一事实的多个来源不能被去重丢失；物化不伪造新增事实的显式来源。
- 额外工作表、索引选择、SQLite 页布局和行序不在契约中。默认图的去重并集语义不变；必须按词项及来源比较，不能比较不同构建的 ID。
- `capabilities.runtime` 提供诊断与读取版本元数据；消费者使用 API / read_contract 协商能力，发行校验另核对实际源码及二进制摘要。

10,000 条固定 RDF 回归同时验证读取表、显式数量、来源引用、变量绑定、规则检查、无超时选项及发布回执的请求所有权，防止上层适配只改命令名而遗漏执行语义。

## 派生查询数据

Full 查询可以提供 `values` 和 `derived_queries`，分别要求 `values-bindings/v1`、`derived-query-data/v1` 能力。`values` 是 `{variables:["s","o"],rows:[[typedTerm,typedTerm]]}` 的表数组，绑定查询内变量列表相同的空 `VALUES (?s ?o) {}`；`null` 表示 UNDEF。所有非 null 单元格均使用同一套 IRI/字面量类型校验，包括当前查询未使用的表；合法表可以只供某条派生查询使用。不插入查询文本。

`derived_queries` 最多 32 条本地 CONSTRUCT 或 SELECT s/p/o 查询，按列表顺序在同一核验过的 Full 只读快照上执行。前序结果是后序查询默认图的去重三元组，不写入 Bundle，也不进入具名图；最终查询仍为 SELECT/ASK。请求只携带查询和 typed VALUES，不接受调用方任意 RDF 请求图。pack 不支持此扩展，能力不足明确失败。

整个执行共享超时、RSS、scratch、取消与回收；累计中间结果受 `max_intermediate_rows` 和 `max_derived_facts` 限制，任何阶段失败均不返回部分结果。查询与参数由外层 `request_sha256` 绑定；查询派生关系不伪装成 materialization 逐事实 Proof。
