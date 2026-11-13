# OntoKiln Pack v4 功能与性能回归基线

## 目标与原则

使用 **同一套冻结输入、同一套独立期望、一个 Release 构建与明确的计时边界** 验证编译、规则物化、Pack 保存/加载、SPARQL、SHACL 的正确性和速度。Pack v4 是唯一生产格式；历史 Full 的基线语料仅作为测试输入和正确性参考，不恢复 Full/SQLite 执行代码。

错误答案必须导致 CI 失败，性能数字只在确认答案正确后发布。公共 GitHub Actions 虚拟机负载会漂移，所以默认 CI 将延迟作为报告和趋势监控；严格速度门槛要求同一固定机器/同一环境进行重复 A/B。

## 测试分层

| 层级 | 输入与方法 | 硬判定 | 计时范围 |
|---|---|---|---|
| 单元/语义 | crates/pack、compiler、reasoner、shacl/engine 的 Rust 测试 | Pack 完整性、规则闭包、绑定语义、具名图、SHACL-SPARQL | 非性能基准 |
| frozen CLI | benchmarks/v4_baseline_regression.py，100K 与 1M | 原冻结生成器的显式/推导公式，七类 SPARQL 精确 RDF 解、Shapes 违规 | 每次 CLI 新进程与 Pack 重载 |
| resident 100K | benchmarks/v4_resident_baseline.py，1 次预热、3 次正式采样 | 同上，并验证编译前/后、持久化前/后的答案 | **一个 Engine 常驻**；逐阶段单独计时 |
| resident 1M | 同上，1 次预热、1 次采样，容量/正确性门禁 | 同上 | 记录单样本；不能由此计算 P95 |
| 生产本体 | 实际 Ontology TTL、Rule IR、SPARQL、SHACL 资产 | 独立 golden cases，视图、RDF 类型、完整 target 解析 | 单机固定 A/B，重复 3 次以上 |

固定生成器是 benchmarks/kernel_baseline.py 中的 `kernel-baseline-orders/v1`；每档包含十条 schema、每个记录十条事实、三条自定义正向规则、RDFS 层级/域/值域/短链。100K 的期望为 86,240 条推导事实，1M 为 862,490 条；**期望来自生成公式而不是从新引擎输出反推**。

每个 SPARQL 对答案的比较含变量列、RDF IRI/字面量类型、语言标签、数值词法、重复行和 ORDER BY 顺序；SHACL 检查四个 Shapes、目标集合大小以及负控制违规。发现任何不一致时整个性能报告失效。

## 常驻基准的时间边界

Rust example: examples/v4_resident_bench.rs。
- `compile_ms`：Source 内容摘要核验 + RDF 解析 + 词典构建 + Pack SPO/POS/OSP 索引。
- `materialize_ms`：规则准备、正向不动点求值、最终闭包组装；独立于 compile。
- `reasoner_init_ms`：从完成的 Pack 创建 Engine，不包含文件读取。
- `save_ms`：完整 Pack v4 写入、数据摘要/原子发布。
- `load_ms`：读取、校验、解码与构造 Engine；只计一次。
- `cold_after_load_ms`：首次读取该 Query 或首次 SHACL 校验（可能构建数据图/缓存）。
- `shacl_focused`：固定 ProbeShape + 单个 focus，衡量局部校验的首次与常驻热耗时；不得与全目标 SHACL 的规模线性扫描相混淆。
- `hot.{query,shacl}.median_ms`：同一 Engine 对象预热后进行的 N 次实际请求；不重新 load_pack。
- `memory.*`：Linux /proc/self/status 中的当前 RSS 和进程高水位 RSS；跨进程和 mmap 共享页不能简单相加，也不等于 PSS。

**不要将 CLI 每请求 Pack 加载的查询时间与 resident 热查询时间相比较。不要将 Debug 与 Release 结果直接除法宣称优化收益。**

## 可复现命令

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

cargo build --release --locked --example v4_resident_bench

# 常驻热查询、常驻 SHACL：100K 3 样本
python3 -B benchmarks/v4_resident_baseline.py \
  --facts 100000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench \
  --output target/v4-baselines/100k.json

# 100 万事实与 862,490 条完整推导事实
python3 -B benchmarks/v4_resident_baseline.py \
  --facts 1000000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench \
  --output target/v4-baselines/1m.json

# 同机前后对比，必须是同一生成器、规模和查询集；只有
# 在固定机器和明确容忍度下才启用 hard regression gate。
python3 -B benchmarks/v4_resident_baseline.py \
  --facts 100000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench \
  --output target/v4-baselines/100k-after.json \
  --compare target/v4-baselines/100k-before.json
```

可选使用 `--max-regression-ratio 1.25` 在控制宿主上拒绝超过 25% 的耗时回退；GitHub 公共 Runner 默认只报告性能数字，不按绝对时延硬拒绝。CI 运行结果存放于 GitHub Actions `v4-resident-benchmark` 工件，含完整 JSON，包括各样本、基线身份、内核阶段时间、冷/热时延、目标数量和 RSS。

## 历史参考值与已知问题

2026-11-13 的固定 Full 46e0fb5 Release 1M 语料中，编译中位 8.767 秒，规则物化中位 11.046 秒（不同内核和机器，不能直接比较）。新的 Pack v4 在另一个 GitHub Runner 的相同生成器中：1M Build + Save 6.951 秒（另一次 9.908 秒），推导 862,490 条且七类查询结果正确。优化最长索引绑定前的 1M join / aggregate 为 3.254 / 30.353 秒，优化后单次 CLI 分别为 0.421 / 0.606 秒；仍然包含 Pack 重载并受 Runner 负载影响。这些结果作为历史审计证据，不是固定性能承诺。

相关：benchmarks/kernel-baseline/46e0fb5-scaling.md、benchmarks/v4_baseline_regression.py、tests/v4_scale.rs、docs/core-v4.md。适配 Prompt：docs/adapters/faas-codex-prompt.md 与 docs/adapters/local-codex-prompt.md。

## 性能优化纪律

1. 先固定用例和预期并得到优化前 JSON。
2. 定位高成本阶段：词典/规则物化、Pack 读取、索引 join、SHACL 数据图构造与本地 SPARQL。
3. 小范围修改并证明完整 RDF 语义不变；绝不为了“更快”缩小 SHACL focus、忽略规则、裁剪变量或静默丢弃中间结果。
4. 同机重复运行优化后 JSON，检查 `--compare` 输出；保留所有回退和未通过测试，不能选择性汇报最快样本。
5. 如果只是公共 CI 噪声，在报告中注明，避免从单次读数推断加速比。
