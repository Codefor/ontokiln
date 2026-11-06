# 配送组件字段血缘与实现选择实验

本次实验手工建模了一个结算页的三个视图属性，用 3 份 TTL、67 条去重三元组表达字段加工、公共实现、业务线覆盖、可用性降级和徽标扩展。字段与分支均来自仓库内的合成 Go 示例。每个加工或选择节点通过函数符号定位依据，映射由人工编写。[Go 样例](checkout.go) · [词表](ontology/vocabulary.ttl) · [字段事实](ontology/lineage.ttl) · [实现选择事实](ontology/variants.ttl)

## 实测结果

| 问题 | 观察到的答案 |
|---|---|
| 摘要地址由什么生成？ | `Summary.address → AddressVM.text → FormatAddress(Order.city, Order.detail)`。两个字段同属一次格式化加工，分别占用参数 0 和 1 |
| 调用服务就意味着使用了响应字段吗？ | 样例调用 `GeoLookup` 并丢弃返回值。三个视图对 `Geo.debug` 的反向影响集为空 |
| 谁复用了公共地址格式？ | 默认业务中，修改 `Order.city` 可影响摘要和配送面板。合作方覆盖生效时，仅摘要仍使用该字段 |
| 定制、降级和扩展怎样组合？ | 别名可用时合作方面板取 `Order.alias`，不可用时取 `Order.postcode`。打开徽标后，`PrependBadge` 接收 `Campaign.badge` 和已选中的基础文本 |
| 上下文只有“启用徽标”时能确定什么？ | 徽标字段确定参与。城市、详情、别名和邮编各自只在部分分支中参与。已知是合作方但缺少可用性时，仅别名和邮编是候选 |

**把所有候选分支都当作生效依赖会产生多少误报？** 在 V1～V4 四个场景中，以“场景、源字段”配对计数。同一组 TTL 只改变遍历策略，人工预期共有 6 对。

| 遍历策略 | 命中预期 | 多报 | 漏报 |
|---|---:|---:|---:|
| 沿所有候选实现遍历 | 6 | 14 | 0 |
| 按给定上下文选择分支 | 6 | 0 | 0 |

14 个用例按人工定义的确定集和候选集逐项比较。直接编译 TTL 和加载独立 pack 均通过，包含分支、参数顺序和函数定位的文本轨迹逐字节一致。另在临时副本中把 `PartnerText` 的输入从 `Order.alias` 改为 `Order.detail`，V2、V4、V5、V6 随即失败，其余 10 项仍通过。[预期答案](cases.json)

## 实验怎样得到答案

服务的 `/v1/query` 可以直接给出 `FormatAddress input ?` 的两条证明，以及 `SelectBuilder candidate ?` 的两个候选。单次主语查询只返回该节点的事实与 RDFS 推导。跨节点追踪和按上下文选分支在独立 Rust 实验程序中执行，程序的每一次图读取都调用服务使用的 `Reasoner::query`。[实验程序](../scenario_lab.rs)

实验程序把标记为 `SourceField` 的字段作为终点，保留加工节点及输入顺序。遇到选择节点时读取 TTL 中的布尔条件和两条候选边。对缺失条件穷举可行取值，取各上下文来源集的交集为确定来源，取并集减去交集为候选来源。反向影响通过逐个追踪三个已登记视图计算。本次检验的是合成模型的语义正确性，性能和真实 Go 代码提取准确率需另行测量。

## 复现

在仓库根目录运行以下命令。第一条运行全部用例和两种遍历策略对照，第二条展示“合作方覆盖并启用徽标”的选择依据。

```sh
cargo run --locked --example scenario_lab -- --compare
cargo run --locked --example scenario_lab -- --case V4 --trace
```

也可从 pack 重放同一实验，并在 HTTP 中检查单步事实。

```sh
./target/debug/ontokiln compile --ttl examples/scenarios/ontology --out data/scenarios.pack
./target/debug/examples/scenario_lab --pack data/scenarios.pack --compare
./target/debug/ontokiln serve --pack data/scenarios.pack --listen 127.0.0.1:7879
```

```sh
curl -sG http://127.0.0.1:7879/v1/query \
  --data-urlencode 'subject=urn:ontokiln:lab:FormatAddress' \
  --data-urlencode 'predicate=urn:ontokiln:lab:input' \
  --data-urlencode 'explain=true'
```
