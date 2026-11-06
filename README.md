# OntoKiln

Rust 本体编译与内存推理服务。当前 demo 将一个目录下的 Turtle 文件合并成一个 RDF 图，支持按类继承、属性继承推导事实，并能返回支撑结果的原始三元组。

[配送组件血缘与复用定制实验](examples/scenarios/README.md) 包含合成 TTL、14 个可重放用例，以及按上下文选择实现的对照结果。

## 启动 demo

在仓库根目录运行以下命令。开发验证使用 Rust 1.93.1。

```sh
cargo build --locked
./target/debug/ontokiln serve --ttl examples/ontology
```

这会递归扫描目录中扩展名为 `.ttl` 的普通文件（忽略大小写，跳过符号链接），在内存中编译一次，然后监听 `127.0.0.1:7878`。可将 `examples/ontology` 换成自己的本体目录。解析错误会报告文件名并中止启动。相对 IRI 按各文档的文件 URL 或显式 `@base` 解析，同名空白节点按文档隔离。`owl:imports` 作为显式事实保留，需要纳入推理的本体文件应放在扫描目录内。

若要把编译和服务部署分开，先通过 Ctrl-C 停止上面的进程，再执行下面两条命令。

```sh
./target/debug/ontokiln compile --ttl examples/ontology --out data/demo.pack
./target/debug/ontokiln serve --pack data/demo.pack
```

可将生成的 pack 单独拷贝到服务机器。加载时检查格式版本、SHA-256、字典顺序和索引边界，校验成功后才开始监听。两种启动方式共享同一个只读推理内核，加载完成后查询只访问内存。使用 `--listen 127.0.0.1:7879` 可另选端口。要更新数据，重新编译并重启服务。

## 查询与推导依据

示例中显式声明了 `delivery readsField OrderDeliveryText`，且 `readsField` 是 `dependsOn` 的子属性。下面的查询会返回由这两条事实推出的依赖关系。

```sh
curl -sG http://127.0.0.1:7878/v1/query \
  --data-urlencode 'subject=https://example.org/shop/delivery' \
  --data-urlencode 'predicate=https://example.org/shop/dependsOn' \
  --data-urlencode 'explain=true'
```

返回的 `object` 为 `<https://example.org/shop/OrderDeliveryText>`，`inferred` 为 `true`。证明的第一步为原始字段关系，第二步为使该关系成立的子属性公理。若同一事实也在 TTL 中显式声明，优先返回显式依据。每项返回一条证明。

| 接口 | 输入与返回 |
|---|---|
| `GET /health` | 就绪状态、启动来源（`ttl` 或 `pack`）、推理规则版本、显式三元组及词条计数。仓库样例包含 2 个文件、6 条显式三元组 |
| `GET /v1/query` | 必填 `subject` 和 `predicate`，均为完整 IRI。可选 `explain=true` 返回证明。结果去重并排序，对象采用 N-Triples 词项写法，保留字面量的语言与数据类型。未知 IRI 返回空集合 |

将上例的谓词换成 `http://www.w3.org/1999/02/22-rdf-syntax-ns#type`，可查到显式的 `DeliveryPanel`，以及推导的 `ViewComponent` 和 `Component`。URL 中含有 `#` 的谓词需经过 URL 编码，上述 curl 写法会自动处理。

## 编译、离线数据与推理的边界

| 位置 | 职责 |
|---|---|
| `crates/compiler` | 顺序稳定地扫描并解析文档，编码 RDF 词项，合并重复事实，构建按主语、谓词、对象排序的整数索引 |
| `crates/pack` | 定义离线快照格式及其校验和读写。相邻临时文件写完后原子替换目标 pack。原始样例在 `examples/ontology`，生成产物默认放在 Git 忽略的 `data/` |
| `crates/reasoner` | 在已加载索引上按主语进行有界前向推导，记录一条可回溯证明。该 crate 仅依赖 pack 的内存结构和序列化接口 |
| `src/main.rs` | 提供 `compile`、`serve` 和 HTTP 参数校验，限制同时执行的查询数量 |

规则集 `rdfs-hierarchy-v1` 读取显式的 `rdfs:subClassOf` 和 `rdfs:subPropertyOf` 层级边，支持多级父类、上位属性、实例类型继承及属性事实提升。按已访问事实去重，循环继承可以终止。本 demo 的范围限于该层级规则集。SPARQL、domain/range、完整 OWL 语义、条件化字段血缘与热切换属于后续工作。

编译与装载均使用内存，单个 pack 上限为 256 MiB。每条查询最多尝试生成 10 万条事实，对象及证明的词项文本预算为 1 MiB。超预算返回 HTTP 422，并发已满返回 429。亿级目标的磁盘溢写、容量预算和压测标准见[后续详细设计](docs/rust-design-v1.md)。

运行 `cargo test --workspace --locked` 可验证两种来源的推导一致性、循环继承、空白节点隔离和损坏 pack 拒绝加载。
