# Local Pack v4 golden

公开的合成模型属性样例，目录与 Source manifest 使用同一组四种格式的文档。`sources.json` 的摘要固定在文件内容上；manifest 路径相对于 manifest 所在目录解析。

- RDFS：`cardTitle` 从 `DisplayProperty` 推导为 `ModelProperty`。
- Property Chain：`computedBy / readsSlot` 推导直接的 `dependsOn`。
- Datalog：传递规则将 `cardTitle → title → rawTitle` 物化到完整闭包。
- Named graphs：`release`、`audit` 保留图身份；SPARQL 默认图与 SHACL 使用闭包的 SPO union。三项规则原子读取物理默认图，不能将 union 当作规则输入。
- 文档空白节点：四份文件都使用 `_:shared`，查询得到四个不同节点。
- SHACL：Core 和 SPARQL 各有通过、失败对照，消费推导关系和具名图数据。
- `golden.json` 固定 SELECT 的列、RDF 类型、排序和 ASK 结果；`tests/v4_local.rs` 也验证 typed parameters、RDF initial bindings、声明为空的 VALUES slot、请求级派生和失败保留旧 Engine。

```bash
cargo build --release --locked --bin ontokiln --example local_reasoning
mkdir -p target/local-golden
./target/release/ontokiln build --sources examples/local/sources.json \
  --rules examples/local/rules.json --profile rdfs-property-chain-v1 \
  --pack target/local-golden/ontology.pack
./target/release/ontokiln query --pack target/local-golden/ontology.pack \
  --query examples/local/queries/lineage.rq
./target/release/ontokiln validate --pack target/local-golden/ontology.pack \
  --shapes examples/local/shapes/core-pass.ttl
./target/release/ontokiln inspect --pack target/local-golden/ontology.pack
cargo test --test v4_local --locked
```

Pack 输出必须是新路径。重复运行时选择新的文件名。`core-fail.ttl`、`sparql-fail.ttl` 预期返回 `conforms:false`，CLI 成功执行校验本身不等于符合 Shapes。

常驻交互例子：

```bash
./target/release/examples/local_reasoning --sources examples/local/ontology \
  --rules examples/local/rules.json --profile rdfs-property-chain-v1 \
  --pack-dir target/local-generations --watch
# stdin 每行一个命令：
query examples/local/queries/lineage.rq
validate examples/local/shapes/sparql-pass.ttl
rebuild
quit
```

`--watch` 默认每秒检查内容摘要，文件新增、删除、内容或规则变化触发全量编译；不使用增量推理。删除必须仍留下至少一份有效 RDF。使用 manifest 时，修改 RDF 后必须更新其声明的摘要；否则重建失败，旧 Engine 继续服务。轮询为串行示例，会读取完整源文件，编译期间请求排队。失败只在输入再次变化或显式 `rebuild` 时重试。标准输出是查询/校验 JSON，状态和错误输出到标准错误。每个成功版本保留独立 Pack 文件，退出后由开发者按需清理。
