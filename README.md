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

## macOS / Linux 本地开发

仅需 Rust 1.95.0；黄金样例和应用启动不需要 Abase、容器或远程服务。首次获取 Cargo 依赖后可离线构建：`cargo build --release --locked --offline`。

四个 CLI 命令保持不变。`--sources` 接受递归 `.ttl/.trig/.nt/.nq` 目录或严格的 JSON `Source` 数组。目录按相对 UTF-8 文件名固定文档顺序；manifest 保留显式顺序，文件路径相对于 manifest 解析。编译器验证每份文件的 SHA-256；空白节点身份以文档为作用域。规则接受已有 Rule 数组或 `ontology-rule-ir/v1`，未知字段、非法规则、不完整闭包、损坏 Pack 都会失败。

完整例子见 [本地黄金样例](examples/local/README.md)，涵盖模型属性血缘、RDFS、Property Chain、Datalog、具名图、参数和 SHACL 正反例。

连续查询使用 [常驻 Rust Library 示例](examples/local_reasoning.rs)：

```bash
cargo build --release --locked --example local_reasoning
./target/release/examples/local_reasoning --sources examples/local/ontology \
  --rules examples/local/rules.json --profile rdfs-property-chain-v1 \
  --pack-dir target/local-generations --watch
```

此例在一个进程内保留 `OntologyEngine`，支持 `query FILE`、`validate [FILE]`、`rebuild`、`quit`。可选 `--watch` 每秒比较源文件/规则内容；有变化后重新全量编译并保存、重载新 Pack，全部成功后替换 Engine。失败时保留旧版本。轮询与编译是串行的，期间请求排队；每一代使用新的 Pack 文件，保留回放版本。应用可直接调用 `local::load_sources/load_rules` 和 `OntologyEngine::build()`，或只调用 `OntologyEngine::load_pack()`，复用同一个 Engine 执行多次 `query()/validate()`。`local` 只提供文件适配，不定义另一套 RDF 或规则执行语义。

本地与 FaaS 消费者共享同一 Engine、Pack、规则 IR、SPARQL 和 SHACL。回归测试验证目录、manifest、直接嵌入、Pack 重载的结果一致；FaaS 发布、存储、路由和实际部署不属于此本地开发例子的验证范围。

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
