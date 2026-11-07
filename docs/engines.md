# 查询与离线规则

| 库 | 可运行入口 | 数据语义 |
|---|---|---|
| oxrdfio / oxrdf | `compile --ttl DIR`、`serve --ttl DIR` | 递归读取 Turtle 和 N-Triples，文档内解析相对 IRI、隔离空白节点，构建显式默认图，并保留每份文档的命名图 |
| spargebra / spareval | `sparql --pack FILE --query FILE.rq`、HTTP 模板 | 使用 pack 的 SPO、POS 和 OSP 索引执行 SELECT、ASK、CONSTRUCT、DESCRIBE。SELECT/ASK 使用 SPARQL Results JSON，图结果使用 N-Triples |
| memmap2 | `serve --pack`、所有读取 pack 的 CLI | 只接受 pack v3；小端平台直接读取词典偏移、词项、SPO 和两张行号表 |
| roaring | `POST /v1/entities` | HTTP 按给定类型和子类从 POS 区间构造请求所需位图。每个类型可以合并其子类的实例，再对请求中的各类型求交 |
| rayon | 编译词项排序、并行重编码、SPO 排序及 `POST /v1/query/batch` | 批量请求按主语独立遍历，按输入次序返回。文件解析保持稳定顺序。用 `RAYON_NUM_THREADS` 设置工作线程数 |
| petgraph | `rules --program FILE` | 按规则体谓词至头谓词构图，折叠强连通分量并拓扑排序。在同一分量内算到不动点，再处理依赖它的规则 |
| datafrog | `rules --engine datafrog`（默认） | 对用户提供的正向二元关系规则进行半朴素求值，输出新增三元组 |
| ascent | `--features ascent-engine` 后使用 `rules --engine ascent` | 用声明式 Datalog 程序执行相同的规则与阶段，可与 Datafrog 逐条对照。仅离线命令使用 |

## Pack v3

编译器与所有 pack 入口统一使用 v3。升级已有部署时，从原始 `.ttl` / `.nt` 重新编译 pack，再重启服务。加载器在复制或解析正文前检查版本，错误信息包含检测到的版本、期望版本和重编提示。可通过 `GET /health` 的 `pack_version` 确认运行版本。

磁盘字段均采用小端序。文件依次包含 8 字节魔数 `ONTOKILN`、4 字节版本号、32 字节正文 SHA-256，随后是正文。正文包含词项数（u32）、三元组数（u64）、相对词项文本起点的字节偏移表（词项数 + 1 个 u64，末项等于文本总长度）、有序 SPO 行（每行三个 u32）、两张行号排列（POS、OSP，分别为每行一个 u32）、每行一个主来源文档 ID、连续 UTF-8 词典文本，以及 JSON 来源尾部（文件元数据与重复断言的其他来源）。偏移表从文件第 56 字节（从零计）开始，偏移表及 SPO 均自然对齐。加载时校验偏移边界、UTF-8、排序和 ID 引用。

## 用同一个 pack 查询

```sh
cargo build --release --locked --features ascent-engine
BIN=./target/release/ontokiln
$BIN compile --ttl examples/scenarios/ontology --out data/scenarios.pack
$BIN sparql --pack data/scenarios.pack --query examples/queries/inputs.rq
$BIN serve --pack data/scenarios.pack --queries examples/queries --listen 127.0.0.1:7880
```

在另一个终端查询某次格式化的两个显式输入。模板名是启动目录内的文件名去掉 `.rq`。绑定值使用 N-Triples 词项语法，通过求值器做变量绑定。下面的结果包含 `Order.city` 和 `Order.detail`。[模板](../examples/queries/inputs.rq)

```sh
curl -sS -H 'Content-Type: application/json' http://127.0.0.1:7880/v1/sparql \
  -d '{"template":"inputs","bindings":{"step":"<urn:ontokiln:lab:FormatAddress>"}}'
curl -sS -H 'Content-Type: application/json' http://127.0.0.1:7880/v1/entities \
  -d '{"types":["urn:ontokiln:lab:SourceField","urn:ontokiln:lab:GoField"],"subclasses":true,"limit":3}'
curl -sS -H 'Content-Type: application/json' http://127.0.0.1:7880/v1/query/batch \
  -d '{"subjects":["urn:ontokiln:lab:FormatAddress","urn:ontokiln:lab:PrependBadge"],"predicate":"urn:ontokiln:lab:input","explain":true}'
```

* **数据集与参数。** 默认查询显式去重并集，显式视图的命名图保留文件边界。支持按版本选取 asserted、inferred、closure 和显式 RDF 参数；见[接口、证明及剖析](query-service.md)。HTTP 仅运行已注册的只读模板；未启用远程 SERVICE。
* **类型过滤。** `subclasses` 默认 false；开启后沿显式子类边反查并处理环。允许 1～32 个类型，返回总数与最多 `limit` 个实体（默认 100，最大 1,000）。HTTP 工作进程查询所选类型的 POS 区间。
* **查询预算。** 所有模式共用最多 2 亿条候选行预算，输出默认 10 万行 / 4 MiB。工作进程的整个求值（含连接、排序、去重、路径）最多 3 GiB Rust 堆、5 秒。普通请求超限返回 422，超时返回 408；JSONL 在首行后用末行错误通知。各上限、指标含义及取消流程见[资源控制](query-service.md#预算取消与流式读取)。
* **映射的生命周期。** 为避免外部写入或截断原文件使映射失效，加载器将输入复制到私有临时文件，完成映射后关闭写句柄，并在提供查询前校验摘要、字典及索引。启动时要预留约一份 pack 的临时磁盘空间。单 pack 默认上限为 256 MiB，可通过进程环境变量 `ONTOKILN_MAX_PACK_MIB` 显式设置。

## 离线规则

程序是 JSON 数组，每个元素含一个 `head` 谓词 IRI 和含一至两个谓词 IRI 的 `body`。单谓词表示 `h(x,y) ← p(x,y)`，双谓词表示按共享变量连接的 `h(x,z) ← p(x,y) ∧ q(y,z)`。允许递归、互递归和多个规则定义同一头谓词。样例先统一输入槽，再求可达的加工输入。定制条件仍由场景实验的选择器解释。[规则样例](../examples/rules/lineage.json)

```sh
./target/release/ontokiln rules --pack data/scenarios.pack --program examples/rules/lineage.json > data/datafrog.nt
./target/release/ontokiln rules --pack data/scenarios.pack --program examples/rules/lineage.json --engine ascent > data/ascent.nt
cmp data/datafrog.nt data/ascent.nt
```

输出按谓词和词项 ID 稳定排序，只写新增事实。以稳定 IRI 标识的结果可连同原数据重新编译，此时这些结果在新 pack 中具有显式事实语义。空白节点的标签只在单份 RDF 文档内有效。需要跨原文档和推导文件关联同一节点时，输入应先为该节点分配稳定 IRI。离线规则包最多 32 条规则，默认允许参与谓词的主宾位置合计出现 256 个不同词项。大型任务可设置 `--max-terms N`。稠密递归与连接可能产生平方规模的结果及更大的中间关系，预算应按所选谓词的实际规模确定。

`rules --proofs out.nt` 同时导出规则版本、前提和来源，支持将该文件单独编译成可查询 pack。RDFS 的对应命令是 `proofs --pack FILE --out out.nt`。版本化输入需指定 `--revision`。[完整示例](query-service.md#选择原始事实推导事实和证明)
