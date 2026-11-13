# Pack v4 本地基线（2026-11-13）

本次测量使用同一个 Release resident Rust example 和冻结的 `kernel-baseline-orders/v1` 生成器，在 macOS arm64 上依次执行。Rust `1.95.0`，target `aarch64-apple-darwin`，18 个逻辑 CPU，`RAYON_NUM_THREADS=1`。100K / 1M 都预热 1 次、热查询和 SHACL 采样 3 次；编译、物化、保存、加载与各项首次请求分别只测量 1 次。

- 代码 SHA：`65259482129a46e9c5d64948e360c1af38d65cd3`。后续提交仅补充性能 JSON 与本文档。
- 100K：100,000 条显式事实、86,240 条推导事实；Pack 8,614,839 bytes。
- 1M：1,000,000 条显式事实、862,490 条推导事实；Pack 86,179,370 bytes。
- 两档均通过 7 项独立 SPARQL RDF 结果/列/排序/重复行校验；四个 Shapes 的全目标计数分别为 20,000 / 200,000，预期一个违规。单 focus 对照无违规。
- 原始 JSON：[100K](../benchmarks/local-v4-baselines/macos-arm64-100k.json)、[1M](../benchmarks/local-v4-baselines/macos-arm64-1m.json)、[100K 同机比较](../benchmarks/local-v4-baselines/macos-arm64-100k-compare.json)。结果包含每次采样、完整查询解、SHACL 覆盖、源与二进制摘要、编译器和宿主身份摘要。

## 分阶段耗时

单位 ms。热查询取 3 次中位数，首次耗时是该查询在 Pack 重载后的首次调用；查询计时含 SPARQL Results JSON 编码和基准的 JSON 解码。

| 阶段 | 100K | 1M |
|---|---:|---:|
| 编译（含摘要、解析、索引） | 146.024 | 1543.631 |
| 规则物化完整闭包 | 139.179 | 1951.519 |
| Engine 初始化 | 4.981 | 4.081 |
| Pack 保存 | 28.582 | 249.769 |
| Pack 加载并校验 | 27.872 | 233.309 |
| 点查询首次 | 0.043 | 0.057 |
| 点查询热中位 | 0.028 | 0.027 |
| 全目标 SHACL 首次 | 539.538 | 7311.320 |
| 全目标 SHACL 热中位 | 293.601 | 3239.963 |
| 单 focus SHACL 首次 | 0.141 | 0.266 |
| 单 focus SHACL 热中位 | 0.100 | 0.122 |

## 各项查询

单位 ms，每格为“首次 / 热中位”。固定点查与受限短路径不能代表生产本体的任意递归血缘查询。

| 查询 | 100K | 1M |
|---|---:|---:|
| `asserted_fact_count` | 7.712 / 7.469 | 78.242 / 83.433 |
| `bounded_property_path` | 0.054 / 0.049 | 0.062 / 0.063 |
| `closure_business_join` | 0.039 / 0.026 | 0.049 / 0.029 |
| `customer_region_join` | 1.121 / 1.050 | 11.311 / 10.453 |
| `inferred_predicate_counts` | 8.428 / 8.283 | 92.485 / 92.616 |
| `point` | 0.043 / 0.028 | 0.057 / 0.027 |
| `region_aggregate` | 10.339 / 10.240 | 102.589 / 106.141 |

## 内存与比较边界

macOS `/usr/bin/time -l` 的整个进程峰值 RSS：100K 421.4 MiB，1M 4059.8 MiB。阶段 RSS 字段为 null。基准同时保留构建的 Engine 与重载的 Engine 以比较答案，峰值还含 SHACL 数据图、覆盖目标与结果，不能作为仅驻留一个 Pack 的内存开销。

100K 同一二进制重复运行的 `--compare` 验证了全部 23 个阶段/首次/热指标，未设置绝对速度门槛。单次 Engine 初始化从 4.981 ms 变为 0.438 ms；短测量、分配和宿主调度噪声较明显。这是回归机制和可重复生成结果的验证，不能据此宣称代码加速。

首次 100K 运行同时有容量测试构建，只用于 Release 黄金预检；正式 100K、同机比较与 1M 随后串行运行。此次没有实测 Linux 或真实 FaaS；仓库合同 CI 已加入 macOS/Linux 矩阵，部署语义与生产性能需要各自验证。

## 重现

```bash
cargo test --workspace --locked
cargo build --release --locked --example v4_resident_bench
python3 -B benchmarks/v4_resident_baseline.py --facts 100000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench --output target/100k.json
python3 -B benchmarks/v4_resident_baseline.py --facts 100000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench --output target/100k-after.json \
  --compare target/100k.json
python3 -B benchmarks/v4_resident_baseline.py --facts 1000000 --samples 3 --warmups 1 \
  --binary target/release/examples/v4_resident_bench --output target/1m.json
```

比较只接受同宿主、OS/架构、Rust target/toolchain、Release、线程/预热/采样、生成器、源摘要、规则和查询集一致的报告。源代码与二进制摘要可不同；跨机器、旧报告缺失环境字段会明确失败。详细边界见 [v4 基准说明](v4-baseline.md)。
