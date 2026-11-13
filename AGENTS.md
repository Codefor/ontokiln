# OntoKiln 开发与评审约束

## 当前定位

OntoKiln 是可嵌入的 **Rust 全内存 RDF 编译与推理内核**。当前且唯一支持的持久化格式是 **Pack v4**。无独立 Full/SQLite 引擎，无 Pack v3 兼容，无 HTTP、API-v1、Worker、Session、FaaS/Pod/Abase 治理。不要恢复已删除的历史接口或分支。

应用通过 `OntologyEngine::build(&[Source], profile, &[Rule])` 编译并物化规则，直接得到可查询 Engine；通过 `save_pack` 持久化；通过 `load_pack` 恢复；`query`、`query_bound`、`query_with_derived`、`validate`、`validate_shape_sources` 使用同一只读闭包。构建成功意味着不动点已完成，推导失败不能发布部分闭包。

## 源码职责

| 模块 | 唯一职责 |
|---|---|
| `crates/compiler/src/v4.rs` | 冻结摘要核验，RDF 解析，规范词典、事实和来源 |
| `crates/compiler/src/v4_materialize.rs` | RDFS / Datalog / OWL 属性链等支持的正向规则闭包 |
| `crates/pack/src/v4.rs` | 四元组、SPO/POS/OSP、位图/来源、原子 Pack 保存与完整性校验 |
| `crates/reasoner/src/v4.rs` | 索引查询、SPARQL Dataset 适配及请求级派生 Overlay |
| `src/engine.rs` | 统一内存对象、Pack 生命周期、查询与 SHACL 缓存 |
| `src/rdf_graph.rs` / `src/shacl.rs` | 共享只读数据图、SHACL Core/Property Path/SPARQL |
| `src/main.rs` | `build` / `query` / `validate` / `inspect` 四个本地薄 CLI |

- 不按业务类/属性名/IRI 命名空间进行优化或分支；只实现可解释、通用的 RDF/规则/SHACL 语义。
- RDF 具名图与文档来源分离；默认 SPARQL 图按所选视图做 SPO 去重并集；推导事实不改写显式来源；请求级 derived Overlay 不写回 Pack。
- 确保数据摘要、Pack SHA-256、长度/索引完整性及异常输入检查；不得用输入声明的容量分配未验证的巨大数组。
- 同一 Engine 可以被多线程共享；SHACL 缓存必须只存不可变数据，运行时 focus、规则临时状态和派生事实均请求级隔离。
- 仅在确有业务需求时新增 Rust API；禁止引入数据库或服务端口。部署与分片见 [FaaS 适配任务](docs/adapters/faas-codex-prompt.md)。

## 功能与性能门禁

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked --bin ontokiln
cargo build --release --locked --example v4_resident_bench
python3 -B benchmarks/v4_resident_baseline.py \
  --binary target/release/examples/v4_resident_bench \
  --facts 100000 --samples 3 --warmups 1 --output target/v4-baselines/100k.json
```

CI 的冻结语料 100K/1M：固定 100000/1000000 显式事实，RDFS + 3 条自定义规则，独立期望 86240/862490 推导事实。必须精确核对 SPARQL 解的类型、词法形式、重复行与顺序及 SHACL targets/违规，再发布性能结论。

性能必须区分 compile、materialize、Reasoner init、save、load、first-query 和 resident-hot query/SHACL，单独记录完整 target 与指定 Shape/focus 两种 SHACL。公共 GitHub Runner 性能只是观测，不能与历史不同机器/不同配置数字直接计算“倍速”；固定宿主上三次以上同机 A/B 才允许强性能判断。

[基线合同](docs/v4-baseline.md) · [本地适配](docs/adapters/local-codex-prompt.md) · [核心 API](docs/core-v4.md)。

## 历史证据

`benchmarks/kernel-baseline/` 及旧 Full/SQLite/Pack v3 历史报告作为**只读审计证据**保留，旧脚本不支持当前 Pack v4 CLI，不得当作现行测试入口。当前可执行基线为 `v4_baseline_regression.py` 与 `v4_resident_baseline.py`。评审应同时检查所有新用例的独立 expected oracle，而不能只依赖 CI 绿色状态。
