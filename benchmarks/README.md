# OntoKiln Pack v4 基线

当前可执行性能测试只针对 **Pack v4 全内存引擎**：

```bash
cargo build --release --locked --bin ontokiln --example v4_resident_bench
python3 -B benchmarks/v4_resident_baseline.py \
  --binary target/release/examples/v4_resident_bench \
  --facts 100000 --samples 3 --warmups 1 \
  --output target/v4-baselines/100k.json
python3 -B benchmarks/v4_resident_baseline.py \
  --binary target/release/examples/v4_resident_bench \
  --facts 1000000 --samples 3 --warmups 1 \
  --output target/v4-baselines/1m.json
```

[详细功能/性能基线合同](../docs/v4-baseline.md) · [机器可读历史观测](v4-baseline-observations.json) · [冻结语料回归](v4_baseline_regression.py)。

基线使用 `kernel_baseline.py` 中冻结的 `kernel-baseline-orders/v1` **输入生成器与独立正确性预期**。此历史脚本保留原生 Full/API 部分仅供审计；其旧执行入口不能用于现行 Pack v4。功能结果错误立即阻断性能发布；常驻热查询与每请求 CLI 加载不能混测。公共 CI 虚拟机仅报告，不设置不合理的延迟绝对门槛。

## 历史研究档案（不支持当前 CLI）

以下文件是特定版本与特定主机的原始实验资料，保留供横向分析，不属于 Pack v4 的可执行测试合同，也不代表当前性能：`kernel-baseline/`、`kernel-20261113.md`、`shacl-sqlite-20261113.md`、`shacl-query-sqlite-20261113.md`、`shacl-bind-joins-20261113.md`、`indexed-queries-v3.md` 以及历史 `business_baseline.py` 和相关 Python 测试。不得重建 Full、SQLite、Pack v3 或旧 HTTP 服务来运行这些档案。

业务真实库应通过 OntoKiln Library 的 `OntologyEngine` 统一适配，并从真实 RDF/Rules/Shapes/SPARQL 资产生成相同的黄金答案及 Release 计时记录。
