# Codex Prompt · OntoKiln Pack v4 本地模式适配

你是负责 OntoKiln 本地运行模式的 Rust 工程师。阅读 Codefor/ontokiln PR #1 最新版本、docs/core-v4.md、src/main.rs、src/engine.rs 和 benchmarks/v4_resident_baseline.py；在不引入独立服务内核、不回退 Pack v3/Full 的前提下，为开发者提供直接可靠的本地编译、推理和测试流程。

## 目标
- 与 FaaS 使用**完全相同**的 OntologyEngine、Pack v4 格式、规则物化、SPARQL、SHACL 和错误语义，数据来源改为本地文件系统，不依赖 Abase、Webhook、容器、泳道或环境变量中的远程服务。
- 支持 macOS (Apple Silicon) 与 Linux，本地调试开发体验优先。不要在 OntoKiln 内实现 Git 仓库解析、复杂任务调度或独立多租户服务。
- 直接使用现有四条 CLI：ontokiln build --sources <directory> --pack <file> --profile <profile> [--rules <json>]，ontokiln inspect --pack <file>，ontokiln query --pack <file> --query <rq> [--view closure]，ontokiln validate --pack <file> [--shapes <ttl>]。
- 交互式或工具内的高频查询应调用 Rust Library 的 OntologyEngine::build / load_pack 后**常驻同一个进程**执行多次 query/validate；不可通过每次调用 CLI 假装测到了热查询性能。

## 开发要求
1. 明确“本地编译”和“本地运行”的边界：编译读取本地 TTL/TriG/NT/NQ 与规则 IR，独立校验每份 Source 的 SHA-256，构建完整闭包并原子保存 Pack v4；本地运行直接 load_pack 且复用不可变 Engine。请提供简单、稳定的 Rust 示例，而不是复制存储引擎。
2. 显式支持源目录扫描和 manifest 两种入口；路径排序、空白节点按文档隔离、相对 IRI/base IRI、显式与推导事实区分、完整闭包等与 FaaS 语义一致。Pack 缺失或损坏时返回明确错误，不静默重新编译或使用旧产物。
3. 可选支持本地开发者触发“重建”命令，但仅全量编译；如需要文件监听，应作为可选薄封装，识别内容 digest 变化后发布新 Pack，在新引擎准备好后再让本地调用者切换，不加入内核增量编译或跨分支共享。
4. 样例组织：一个小型 schema + ABox + Rules + Shapes + SPARQL 测试目录，含至少一次正向推导、具名图、OWL 属性链、SHACL 违规/通过、typed VALUES 和 derived query 的交互例子。用稳定的 golden 期望核对，而不是只检查是否产生输出。
5. 性能流程：固定 Rust release 构建（cargo build --release --locked），运行 benchmarks/v4_resident_baseline.py 的 100K / 1M 历史冻结语料；区分 compile/materialize/save/load 与首次查询/热查询、SHACL 首次/热校验。生成含用例名称、RDF 事实数、规则、源摘要、samples/p50 和 RSS 的 JSON，允许 --compare 同机器报告，不直接将 Debug 与 Release 或不同机器的数字除法得出“加速比”。
6. 不能引入旧 pack、SQLite、HTTP 服务器或 worker/Session 兼容层。保留现有公共 API，不让本地模式成为第二套推理实现。

## 验收
- 从源目录全量 build → save → inspect → load → ASK/SELECT → SHACL 的单次 CLI 闭环正确。
- 同样 Pack 的本地单进程连续查询与 FaaS 的直接 Rust API 调用一致；错误 RDF、错误 rule IR、坏 SHA、坏 Pack、具名图与断言/推导视图都有测试。
- 使用固定基线核对 RDF 词项类型/字面量、重复解和解的顺序、递归规则闭包计数、SHACL target/focus，不仅比较行数。
- 交付可执行代码、README 快速开始、golden fixtures、性能报告与可审查 MR。无重大澄清直接实施，避免引入宏大的运行时框架。
