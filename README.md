# OntoKiln

OntoKiln 是 Rust 实现的**纯内存 RDF 本体编译与推理内核**。唯一的构建产物是 **Pack v4**，不维护 Pack v3、SQLite Full 或其他历史格式的长期兼容层。

- **Build**：加载 Turtle、N-Triples、TriG 和 N-Quads，在内存中完成规则物化，获得常驻 `OntologyEngine`，可同时保存成 Pack v4。
- **Load**：从 Pack v4 恢复不可变本体及其 RDF 具名图、索引、断言/推导事实和来源元数据。
- **Reason**：基于内存 Pack 执行 SPARQL、SHACL Core 及本地约束查询；请求级派生数据不会写回 Pack。

```bash
cargo build --release --bin ontokiln

# 从 RDF 目录编译完整闭包（还可选 --rules ontology-rule-ir.json）
./target/release/ontokiln build --sources ./ontology --pack ./ontology.pack --profile rdfs-supported-v1

# 查询内存中加载的 Pack
./target/release/ontokiln query --pack ./ontology.pack --query ./query.rq --view closure

# 使用 Pack 内的 Shapes 校验，或通过 --shapes ./shapes.ttl 提供外部 Shapes
./target/release/ontokiln validate --pack ./ontology.pack

# 校验 Pack v4 产物
./target/release/ontokiln inspect --pack ./ontology.pack
```

应用嵌入时直接使用 Rust API：

```rust
use ontokiln::engine::OntologyEngine;
let (engine, stats) = OntologyEngine::build(&sources, "rdfs-supported-v1", &rules)?;
engine.save_pack(std::path::Path::new("/tmp/ontology.pack"))?;
let same_engine = OntologyEngine::load_pack(std::path::Path::new("/tmp/ontology.pack"))?;
```

Pack v4 是唯一接受的 Pack 格式，不提供 v3 读取或升级服务。FaaS 负责生命周期、发布和路由，OntoKiln 不承载 FaaS 服务框架。

## 迁移验证

当前 PR 中保留少量独立的 SQLite Full 对照实现，仅用于尚未完成的语义差分和准入验证；**这不是面向消费者的第二套引擎**。Full 的必要能力迁完并通过固定语料、SHACL 与生产规模回归后将删除该临时实现。

主要测试：`cargo test --workspace --locked`、`cargo test --test v4_parity --locked`、`cargo test --test v4_api --locked`、`cargo test --test v4_cli --locked`、`cargo test --test v4_scale --locked -- --ignored`。
