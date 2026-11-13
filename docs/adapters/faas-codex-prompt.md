# Codex Prompt · OntoKiln Pack v4 FaaS 适配

你是负责 FaaS 应用侧本体运行时集成的高级 Rust/Python 工程师。请阅读 Codefor/ontokiln PR #1 最新版本、docs/core-v4.md、src/engine.rs 和 benchmarks/v4_resident_baseline.py。直接实施应用侧适配，原则是不修改 OntoKiln 的核心架构，也不恢复 Full、SQLite、Pack v3、历史 API 或 Worker/Session。

## 目标和接口
- OntoKiln 是纯 Rust **全内存** RDF 编译、正向规则物化、SPARQL 和 SHACL 引擎。核心 API：OntologyEngine::build(&[Source], profile, &[Rule]) 返回 (OntologyEngine, Materialization)；engine.save_pack(path)；OntologyEngine::load_pack(path)；engine.query(sparql, View)；engine.validate(...) 与 engine.validate_shape_sources(...)。
- build 成功必须意味着完整物化闭包已达不动点。编译后直接使用返回的同一个 engine，无须重载刚写好的 Pack。
- Pack v4 是唯一产物。切勿实现 Pack v3 升级、SQLite Full 兼容、增量编译、跨分支共享缓存、FaaS 感知的 OntoKiln 内核。
- 参考实际规模约 100 万显式 RDF 事实与若干规则、SPARQL、SHACL，不设人为 Pack 体积上限；但必须监控 Pod 8C32G 的真实内存使用并在超出宿主能力时准确失败。

## 实施范围：应用适配层
1. 审查现有 FaaS 入口及 Python/Rust 进程模型，明确每 Pod 的引擎实例归属。优先应用内直连 Rust Library；如果宿主是 Python，可用极薄 PyO3/maturin 绑定或现有 FFI，把 Pack v4 对象保留在长生命周期 Rust 实例中，释放 GIL 后执行只读查询。**不得每个请求启动 CLI、重建 Engine 或反序列化 Pack**。多 Python 进程不能假装共享一个堆对象；先确认实际并发模型，再选 1 个实例/worker 或现有进程内统一入口。不要在没有需求时另建 daemon。
2. 编译入口读取本体源文件的精确 Git commit、Sources 的 expected_sha256、Rules profile、显式 Rule 列表和 SHACL 资产，调用 build()，验证闭包已完成、Pack 元数据、必要 golden queries 和 SHACL 正负控制；成功后保存不可变 Pack。
3. 使用现有 Abase 持久化：按不超过 4 MiB 的块切 Pack，记录块顺序、大小、SHA-256、整个 Pack 的 SHA-256、format=v4、编译器源码/版本、源 Git commit、规则 recipe digest 等；先写数据块和 manifest，校验完成后才原子发布可见 ref。Abase 单分片硬上限是 5 MiB，本方案按 4 MiB 预留余量。不得因上传中断发布不完整 Bundle。
4. 每个 FaaS Pod 独立加载所需 bundle：下载、聚合、逐块与整包校验、load_pack()、执行代表性预热查询，完成后该 Pod 才将对应 ref 标记 READY。新旧 engine 通过应用侧 Arc/ArcSwap 或等价机制原子交换；在途请求继续持有旧 Arc，完成后释放。各 Pod 自己预热和切换，不要求其他 Pod 同时就绪，也不复用其他泳道的内存 Engine。
5. 每个 Git push 产生独立可追溯 RuntimeBundle；默认 prod webhook 的编译流程和同分支泳道手动编译可得到不同 Bundle（由编译配置/版本/环境和源 commit 一起标识），避免相同 commit 错误覆盖部署产物。编译失败保留旧 ref，报告失败原因，不静默降级。
6. 查询与 SHACL 走直接内存 API，明确 View::Asserted / Inferred / Closure、真实 RDF 具名图、默认图 union、typed VALUES、rdf_bindings 与请求级 derived_queries 的调用合同。不要跨请求缓存或写回 derived overlay。
7. 日志/指标至少包含 source commit、Pack SHA、runtime ref、load_ms、warmup_ms、build_ms、materialize_ms、SPARQL/SHACL duration、并发数、Pod 当前/峰值内存、查询成功与错误分类。不要把 Pack 加载时间混入热查询耗时。

## 验收
- 真实 100 万规模 Pack：编译保存、Abase 4 MiB 分片、断点/丢块/坏块校验、重新加载、数据源摘要和规则语义一致。
- 代表性 SPARQL（点查、join、聚合、路径、参数绑定）与 SHACL Core + SPARQL 使用已有 golden case，对返回 RDF 类型、重复解、具名图、违规目标执行精确校验。
- 同一个 Pod 连续 100 次请求只加载一次 Pack（除主动切换）；多线程查询不出现相互污染；ref 切换期间老请求不丢失；新 ref 预热失败不能接流量。
- 兼容测试只针对现行 Pack v4 的核心语义，不补旧 Full/API v1 适配层。
- 提交可审查 MR，交付 Rust/Python 适配代码、部署配置、运行测试、指标口径、失败恢复说明以及执行的 CLI 命令和结果。**仅修改 FaaS 应用仓库，除非发现 OntoKiln 真正缺失的公共能力；此时先在 RFC 说明契约缺口。**
