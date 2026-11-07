# 查询、赋值血缘与证明

编译出的 pack v3 同时保存事实、三种访问顺序和来源。在线服务既可直接读取 TTL，也可只部署 pack。以下命令从仓库根目录执行，使用 `cargo build --release --locked` 生成的二进制。

## 查一个组件的赋值链

```sh
./target/release/ontokiln serve --ttl examples/assignment/ontology \
  --queries examples/assignment/queries --query-slots 2
```

另开终端，从组件开始查询。参数名对应模板里的 RDF 词项 `<urn:ontokiln:param:component>`。

```sh
curl -sS -N http://127.0.0.1:7878/v1/sparql -H 'Content-Type: application/json' \
  -d '{"template":"assignment","request_id":"panel-v2","format":"jsonl",
       "parameters":{"component":{"type":"iri","value":"urn:assignment:Panel"}},
       "options":{"revision":"v2","profile":true}}'
```

[模板](../examples/assignment/queries/assignment.rq)从绑定取目标属性，沿 `assignment/previous*` 找到每个赋值，再取输入、条件、读写阶段、顺序和声明的 `prov:wasDerivedFrom`。结果还含 RDF 命名图，以及源码模块、路径、行号。样例的两个版本复用相同的组件、绑定和赋值 IRI：

| 所选版本 | 第 1 步：模型读取 | 第 2 步：组件写入 |
|---|---|---|
| v2 | `model` 的 `load` 读取 `displayName`，来自 `v2/model.go:3` | `view` 的 `render` 以 `load` 为输入，在 `enabled` 条件下写 `title`，来自 `v2/panel.go:3` |
| v1 | 同一位置读取 `legacyCode`，来自 `v1/model.go:3` | 写入来自 `v1/panel.go:3` |

系统读取 RDF 已声明的关系和条件，不从 Go 源码提取赋值，也不把候选条件当作已满足。源码用于核对身份。原有[业务分支实验](../examples/scenarios/README.md)另行验证条件选择。

每个 RDF 文件是一个命名图；`GET /v1/sources` 列出其确切 graph IRI、相对路径、模块及摘要。没有清单时，模块默认取相对父目录。可用 `GRAPH ?document { … }` 查单条断言的所有文件；显式默认图是所选文件的去重并集。[sources.json](../examples/assignment/ontology/sources.json) 用 **RDF 相对路径**作键；每项须给出 `module`、`revision`、`source_file`、`ttl_sha256` 和 `source_sha256`。摘要是实际文件字节的 SHA-256 小写十六进制；源码必须能在输入目录内解析。错版、摘要不符、清单引用不存在的 RDF 文件都会中止编译，保留旧 pack。清单声明的是关联关系，不能证明人工标注的业务语义正确。

只要 pack 含版本化文档，查询就必须选一个存在的版本。普通模式、命名图、类型过滤、层级推导及每个证明前提都只读该版本；没有版本标签的文件也不会混入。跨版本共享的模式公理应在需要它的版本里显式声明。线上加载 pack 后无需源码、TTL 或清单。

## RDF 参数、索引与执行剖析

推荐在模板的三元组、路径、`VALUES`、`FILTER`、`EXISTS` 或子查询中显式放置 `<urn:ontokiln:param:NAME>`。服务解析模板后在代数树上绑定词项，再进行优化。变量 `?NAME` 仍遵守 SPARQL 作用域；即使与参数重名，也不会被隐式替换。绑定示例：

| RDF 词项 | parameters 中的值 |
|---|---|
| IRI | `{"type":"iri","value":"urn:assignment:Panel"}` |
| 普通字符串 | `{"type":"literal","value":"含引号的文本也只是值"}` |
| 带语言的字符串 | `{"type":"literal","value":"名称","language":"zh"}` |
| 有类型的字面量 | `{"type":"literal","value":"42","datatype":"http://www.w3.org/2001/XMLSchema#integer"}` |

多传、缺失、非法 IRI、在谓词位置传入字面量，或同时传 `language` 和 `datatype`，都会被拒绝。原有 `bindings` 仍用于 SPARQL 的投影变量替换，值使用 N-Triples 词项语法。HTTP 仅运行服务启动时从受信任目录注册的模板。

显式图的 SPO、POS、OSP 三个排序覆盖任意已绑定 S/P/O 组合；二分定位匹配区间，再遍历候选行。连接右侧是有共享键的基本三元组模式时，执行器可逐条用左侧已知值探测索引。一般表达式、聚合、可选分支、路径和笛卡尔积仍遵守原 SPARQL 语义，不保证每种写法都有选择性。层级推导视图按主语生成闭包，未给定主语时可能需要遍历整个图。

* `options.counters=true`：报告全请求 `candidate_rows`、SPO/POS/OSP 各自读取的候选行、输出行数、结果负载字节数和工作进程堆峰值。二分定位的比较不计候选行；同一行被不同模式读取会重复计数。
* `options.profile=true`：增加 `execution.plan` 算子树。节点的 `number of results` 是实际产出行数；`sample_bindings` 最多保留前两行、每行八个变量，每个值截到 128 个字符。节点的扫描量、耗时和 `allocated_bytes` 包含子节点，因此不能相加。分配字节是累计申请量，不代表节点同时占用的内存。
* `peak_heap_bytes` 是工作进程从预算激活到生成统计时观测到的 Rust 堆高水位增量；进程启动、堆分配器开销、线程栈、共享 pack 映射、内核缓冲与父进程不计入。最终封装响应也受硬上限约束。剖析会增加开销，比较正常延迟时关闭它。

## 选择原始事实、推导事实和证明

| view | 默认图 |
|---|---|
| `asserted` | 所选版本的原始断言；SPARQL 默认值。可查询文件命名图 |
| `inferred` | 只返回通过 RDFS 层级规则新增的事实 |
| `closure` | 原始断言和推导事实去重并集；单主语和批量接口的默认值 |

后两种视图不伪造文件命名图。某个推导事实也有显式声明时，视为显式事实。用 `GET /v1/query?subject=…&predicate=…&view=inferred&explain=true` 取一条确定性的证明；版本化数据另传 `revision`。每步给出规则名、规则版本、结论、本步使用的原始前提及其所有可见来源；以后续步骤用到上一结论时还给出 `previous_fact`。

需要用 SPARQL 查询证明结构时，先离线导出并编译成独立快照。示例从配送组件的上位属性结论逆向查询：

```sh
BIN=./target/release/ontokiln
mkdir -p data/proof-input
$BIN compile --ttl examples/ontology --out data/shop.pack
$BIN proofs --pack data/shop.pack --subject https://example.org/shop/delivery --out data/proof-input/evidence.nt
$BIN compile --ttl data/proof-input --out data/proof.pack
$BIN sparql --pack data/proof.pack --query examples/assignment/queries/premises.rq \
  --parameters '{"subject":{"type":"iri","value":"https://example.org/shop/delivery"},
    "predicate":{"type":"iri","value":"https://example.org/shop/dependsOn"},
    "object":{"type":"iri","value":"https://example.org/shop/OrderDeliveryText"}}'
```

输出包括显式的 `delivery readsField OrderDeliveryText` 和 `readsField subPropertyOf dependsOn` 及各自的文件。证明图用 `urn:ontokiln:proof:` 词汇表示事实、规则、前提边和来源，`premise+` 可逐级回溯。对版本化业务数据使用 `proofs --revision v2`。省略 `--subject` 会遍历整个图，建议先估算输出规模。离线 Datafrog / Ascent 也可用 `rules --program RULES.json --proofs evidence.nt` 保存证明；规则版本含程序摘要。当前保存一条可回溯到原始事实的证明，并不枚举所有可替代的推导路径。

## 预算、取消与流式读取

所有在线推理请求使用可终止的独立进程，读取服务已验证的只读快照。截止时间包含启动工作进程、求值、序列化和向服务传输数据；客户端断开、主动取消、超时都会终止并回收进程，之后才归还查询槽。排序、连接、DISTINCT、属性路径及没有 RDF 扫描的纯计算同样受堆和时间限制。默认并发槽为可用核数和 8 的较小值，可用 `serve --query-slots 1..8` 设置。共享映射和系统文件缓存可能继续驻留。

| 限制 | 默认 / 最大 | 请求位置 |
|---|---|---|
| 候选行 | 200,000,000，含最后一行 | SPARQL `options.max_visits` |
| 输出行 / 字节 | 100,000 / 4 MiB；JSONL 可显式提高到 1,000,000 / 64 MiB | `options.max_rows` / `options.max_bytes` |
| 工作堆 | 3 GiB，可调低 | 顶层 `memory_bytes` |
| 时间 | 5,000 ms，可调低，最小 1 ms | 顶层 `deadline_ms` |

单主语、批量、类型交集使用顶层 `max_visits`、`max_rows`、`max_bytes`，输出普通 JSON；原有单主语 10 万步、1 MiB 结果词项文本限制仍生效。类型交集通过 POS 读取所选类型和子类，按请求构建所需位图。使用短生命期工作进程有固定启动成本，大量点查可优先采用批量接口。

客户端可事先指定 `request_id`，执行中用 `DELETE /v1/queries/{id}` 取消（SPARQL 也接受 `/v1/sparql/{id}`）；正常响应头含 `X-Query-ID`。已接受取消返回 202，目标请求返回 409；无活动请求返回 404。超限为 422、超时为 408、槽满为 429；无效 RDF 参数或版本为 400。

SPARQL 设 `format:"jsonl"` 会返回 `application/x-ndjson`。SELECT 每行一个 `{"bindings":{…}}`，词项保留 N-Triples 写法；ASK 返回 `{"boolean":…}`，图结果返回 `{"triple":…}`。有界队列向慢客户端传递背压。**只有读到最后的 `{"end":true,…}` 才是完整成功**。若首行尚未发出，HTTP 可直接返回错误状态；已开始发送后，HTTP 状态无法修改，末行改为 `{"error":"…","status":422}` 等，连接中断则可能没有末行。预算含成功结束记录和剖析包装；独立的出错通知不计入结果限额。CLI 对应 `sparql --jsonl --options '{…}' --parameters '{…}'`。
