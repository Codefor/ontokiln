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

## 验证

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --test v4_scale --locked -- --ignored --nocapture
```

最后一项对 100 万条显式 RDF 事实执行内存编译、Pack v4 保存与加载、SPARQL COUNT 验证。旧 Pack v3、SQLite Full、HTTP/Worker/Session API 不构成公开兼容范围。

## 基线与适配

- [功能/性能回归基线](docs/v4-baseline.md)：冻结 golden 语料、Release 单进程常驻推理、冷热查询、编译物化、SHACL 及指标回归。
- [FaaS 适配开发 Prompt](docs/adapters/faas-codex-prompt.md)：应用侧 Abase Bundle、每 Pod 加载/就绪/切换与进程模型。
- [本地模式开发 Prompt](docs/adapters/local-codex-prompt.md)：macOS/Linux 单实例 CLI 和 Rust 常驻开发工具。
