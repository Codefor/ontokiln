# OntoKiln · 单一 Pack v4 内存内核

OntoKiln 只提供 **RDF → 内存编译/规则物化 → Pack v4 + 常驻内存推理引擎**。应用自己维护 `OntologyEngine` 对象、Pack 的持久化位置和 FaaS 生命周期。仓库不提供独立的 SQLite Full、Pack v3、HTTP 服务、进程 Worker 或跨 Pod 路由。

## 核心 Rust API

```rust
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::Source;
use ontokiln_compiler::v4_materialize::Rule;
use ontokiln_pack::v4::View;

// Source: path / document_id / format / expected_sha256 / base_iri
let sources: Vec<Source> = get_sources();
let rules: Vec<Rule> = get_rules();
let (engine, stats) = OntologyEngine::build(&sources, "rdfs-supported-v1", &rules)?;

// 编译完成即持有可查询的推理引擎；保存 Pack 不会清除/重新加载引擎。
engine.save_pack(std::path::Path::new("/data/ontology.pack"))?;
let answers = engine.query("SELECT ?s WHERE { ?s a <urn:Item> }", View::Closure)?;
let report = engine.validate(None, None, None, 64)?;

// 后续新进程恢复相同的 Pack 状态
let loaded = OntologyEngine::load_pack(std::path::Path::new("/data/ontology.pack"))?;
```

`build` 完成所有受支持的正向规则物化，只有到达不动点才返回成功，不会把部分闭包发布为完整 Pack。`Source.expected_sha256` 在编译前后校验；`save_pack` 原子发布新文件且不覆盖已有文件；`load_pack` 校验版本、SHA-256、结构、词项和索引。

## Pack 数据与推理语义

- Pack v4 由 RDF 词典、四元组（graph, subject, predicate, object）、SPO/POS/OSP 索引、显式/推导位图、来源映射和完整性元数据构成。词项 ID 只在当前 Pack 内有意义。
- RDF 具名图与来源文档独立管理：原始 `GRAPH` 查询保留图名；SPARQL 默认图是可见四元组的去重三元组并集。
- `View::Asserted` 仅断言；`View::Inferred` 仅新增推导；`View::Closure` 是两者并集。
- SHACL 校验的 Data Graph 是所有已存储四元组的 SPO 去重并集（显式 + 推导，跨具名图），不依赖查询视图。可以使用 Pack 内的 Shapes，也可以按摘要核验加载外部 Shapes。
- 可用规则 profile：`no-rules-v1`、`datalog-v1`、`rdfs-supported-v1`、`rdfs-property-chain-v1`。规则的三元组 Atom 固定作用于物理默认图，四元组 Atom 可显式指定图；SPARQL 默认图并集不会改变规则求值范围。
- 本地 SPARQL 支持 SELECT/ASK、显式参数、初始 RDF 变量绑定与 VALUES。CONSTRUCT/SELECT 派生查询仅产生本次请求的 Overlay，不写回 Pack，也不加入具名图。
- SHACL 支持内核已实现的 Core、Property Path 与本地 SHACL-SPARQL/自定义目标；不承诺 SHACL JS、任意 OWL 推理或外部 I/O。

## FaaS 边界

FaaS 保存 `OntologyEngine` 对象即可跨请求复用。何时从 Abase 下载 Pack、何时发布新的 Git ref、何时切换 Pod 均由应用决定；OntoKiln 不承担 Webhook、Pod 预热、路由、进程调度或网络服务。

## 验收标准

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --test v4_scale --locked -- --ignored --nocapture
```

百万级合成 N-Triples 验收样本：1,000,000 个显式 RDF 事实、规则 profile 为 `no-rules-v1`、Pack 63,127,217 字节。一次 GitHub Actions Runner 实测：构建 34.344 秒、保存 2.535 秒、重载 4.485 秒。该结果仅是固定合成负载的功能与容量基线，不应外推到真实业务本体或多规则工作负载。
