//! SHACL Core plus local SELECT constraints from the pinned shapes graph.
//! Validation is called only inside the killable Rust worker.
use crate::rdf_graph::{Graph, PreparedQueries, RDF, SH, iri, value};
use anyhow::{Result, ensure};
use rudof_rdf::rdf_impl::InMemoryGraph;
use rudof_rdf::{rdf_core::RDFFormat, rdf_impl::ReaderMode};
use serde_json::{Value, json};
use shacl_ir::compiled::schema_ir::SchemaIR;
use shacl_validation::focus_nodes::FocusNodes;
use shacl_validation::shacl_engine::{engine::Engine, native::NativeEngine};
use shacl_validation::shape_validation::Validate;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn validate_with_depth(
    data: &Graph,
    shapes: &Graph,
    selected: &BTreeSet<String>,
    focus: Option<&str>,
    max_depth: usize,
    full: bool,
) -> Result<Value> {
    let work_budget = if full { usize::MAX } else { 4096 };
    ensure!(max_depth > 0, "invalid SHACL validation depth");
    ensure!(
        full || shapes.triples.len() <= 100_000,
        "SHACL shape budget exceeded"
    );
    let supported = "targetClass targetNode targetSubjectsOf targetObjectsOf target targetType path inversePath alternativePath zeroOrMorePath oneOrMorePath zeroOrOnePath property node class datatype nodeKind minCount maxCount minExclusive minInclusive maxExclusive maxInclusive minLength maxLength pattern flags languageIn uniqueLang equals disjoint lessThan lessThanOrEquals not and or xone in hasValue closed ignoredProperties qualifiedValueShape qualifiedMinCount qualifiedMaxCount qualifiedValueShapesDisjoint deactivated severity message name description order group defaultValue select ask prefixes declare prefix namespace parameter optional returnType validator sparql labelTemplate";
    for t in &shapes.triples {
        if let Some(local) = value(&t[1]).strip_prefix(SH) {
            ensure!(
                supported.split_whitespace().any(|p| p == local),
                "unsupported Shapes predicate: {}",
                t[1]
            );
        }
        ensure!(
            t[1] != iri("http://www.w3.org/2002/07/owl#imports"),
            "remote Shapes imports are unsupported"
        );
    }
    let queries = data.prepare_queries()?;
    let mut foci = BTreeMap::new();
    for shape in selected {
        ensure!(
            shapes.has(
                shape,
                &iri(&format!("{RDF}type")),
                &iri(&format!("{SH}NodeShape"))
            ) || shapes.has(
                shape,
                &iri(&format!("{RDF}type")),
                &iri(&format!("{SH}PropertyShape"))
            ),
            "selected SHACL shape is unavailable"
        );
        let mut nodes = BTreeSet::new();
        if let Some(focus) = focus {
            nodes.insert(focus.to_owned());
        } else {
            nodes.extend(shapes.objects(shape, &iri(&format!("{SH}targetNode"))));
            for class in shapes.objects(shape, &iri(&format!("{SH}targetClass"))) {
                for node in data.triples.iter().map(|t| &t[0]).collect::<BTreeSet<_>>() {
                    if data.has_type(node, &class) {
                        nodes.insert(node.clone());
                    }
                }
            }
            for p in shapes.objects(shape, &iri(&format!("{SH}targetSubjectsOf"))) {
                nodes.extend(
                    data.triples
                        .iter()
                        .filter(|t| t[1] == p)
                        .map(|t| t[0].clone()),
                );
            }
            for p in shapes.objects(shape, &iri(&format!("{SH}targetObjectsOf"))) {
                nodes.extend(
                    data.triples
                        .iter()
                        .filter(|t| t[1] == p)
                        .map(|t| t[2].clone()),
                );
            }
        }
        if focus.is_none() {
            for target in shapes.objects(shape, &iri(&format!("{SH}target"))) {
                let mut declarations = BTreeSet::new();
                if shapes.has_type(&target, &iri(&format!("{SH}SPARQLTarget"))) {
                    declarations.insert(target.clone());
                }
                declarations.extend(
                    shapes
                        .objects(&target, &iri(&format!("{RDF}type")))
                        .into_iter()
                        .filter(|kind| {
                            shapes.has_type(kind, &iri(&format!("{SH}SPARQLTargetType")))
                        }),
                );
                ensure!(!declarations.is_empty(), "unsupported SHACL custom target");
                for declaration in declarations {
                    let text = value(&shapes.one(&declaration, &iri(&format!("{SH}select")))?);
                    let text = prefixed_query(shapes, &declaration, &text)?;
                    crate::query::local_select(&text)?;
                    let bindings = if declaration == target {
                        vec![BTreeMap::new()]
                    } else {
                        parameter_bindings(shapes, &declaration, &target, true)?
                    };
                    for bindings in bindings {
                        for row in queries.query_bindings(&text, &bindings)?.1 {
                            nodes.insert(
                                row.get("this")
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("SPARQL target must return this")
                                    })?
                                    .to_string(),
                            );
                        }
                    }
                }
            }
        }
        ensure!(nodes.len() <= work_budget, "SHACL focus budget exceeded");
        foci.insert(shape.clone(), nodes);
    }
    // Remove unrelated targets while retaining the selected shape dependency
    // closure. Rudof executes Core; published sh:sparql SELECTs execute below.
    let mut reachable = selected.clone();
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    let links = [
        "property",
        "target",
        "node",
        "not",
        "and",
        "or",
        "xone",
        "qualifiedValueShape",
        "path",
        "inversePath",
        "alternativePath",
        "zeroOrMorePath",
        "oneOrMorePath",
        "zeroOrOnePath",
        "in",
        "languageIn",
        "ignoredProperties",
        "sparql",
        "prefixes",
        "declare",
        "targetType",
        "parameter",
        "validator",
    ];
    while let Some(node) = pending.pop() {
        for t in shapes.triples.iter().filter(|t| t[0] == node) {
            if (t[2].starts_with("_:") || links.iter().any(|p| t[1] == iri(&format!("{SH}{p}"))))
                && (t[2].starts_with("_:") || t[2].starts_with('<'))
                && reachable.insert(t[2].clone())
            {
                pending.push(t[2].clone());
            }
        }
    }
    // Parameter predicates link custom constraints implicitly to their shapes.
    for component in shapes.subjects(
        &iri(&format!("{RDF}type")),
        &iri(&format!("{SH}ConstraintComponent")),
    ) {
        let mut pending = vec![component];
        while let Some(node) = pending.pop() {
            if reachable.insert(node.clone()) {
                pending.extend(
                    shapes
                        .triples
                        .iter()
                        .filter(|t| {
                            t[0] == node
                                && (t[2].starts_with("_:")
                                    || links.iter().any(|p| t[1] == iri(&format!("{SH}{p}"))))
                        })
                        .map(|t| t[2].clone()),
                );
            }
        }
    }
    for t in &shapes.triples {
        if reachable.contains(&t[0]) {
            ensure!(
                t[1] != iri(&format!("{SH}js")) && t[1] != iri(&format!("{SH}rule")),
                "SHACL JS/rules are outside inference=none, advanced=false validation"
            );
        }
    }
    let mut result = validate_composed(
        data,
        &queries,
        shapes,
        &reachable,
        &foci,
        max_depth,
        work_budget,
    )?;
    result["coverage"] = json!(foci.iter().map(|(shape,nodes)|json!({"shape":value(shape),"target_count":nodes.len(),"focus_nodes":nodes})).collect::<Vec<_>>());
    result["target_count"] = json!(foci.values().map(BTreeSet::len).sum::<usize>());
    result["kernel"] = crate::identity::kernel();
    Ok(result)
}

fn prefixed_query(shapes: &Graph, constraint: &str, query: &str) -> Result<String> {
    let mut prefixes = BTreeMap::new();
    for source in shapes.objects(constraint, &iri(&format!("{SH}prefixes"))) {
        for declaration in shapes.objects(&source, &iri(&format!("{SH}declare"))) {
            let prefix = value(&shapes.one(&declaration, &iri(&format!("{SH}prefix")))?);
            let namespace = value(&shapes.one(&declaration, &iri(&format!("{SH}namespace")))?);
            ensure!(
                prefix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    && !prefix.chars().next().is_some_and(|c| c.is_ascii_digit()),
                "invalid SHACL prefix"
            );
            ensure!(
                !namespace.chars().any(|c| c.is_control()
                    || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')),
                "invalid SHACL namespace"
            );
            if let Some(previous) = prefixes.insert(prefix.clone(), namespace.clone()) {
                ensure!(
                    previous == namespace,
                    "conflicting SHACL prefix declaration"
                );
            }
        }
    }
    Ok(prefixes
        .iter()
        .map(|(p, n)| format!("PREFIX {p}: <{n}>\n"))
        .collect::<String>()
        + query)
}

fn parameter_bindings(
    shapes: &Graph,
    declaration: &str,
    instance: &str,
    required: bool,
) -> Result<Vec<BTreeMap<String, String>>> {
    let mut combinations = vec![BTreeMap::new()];
    let parameters = shapes.objects(declaration, &iri(&format!("{SH}parameter")));
    ensure!(
        !parameters.is_empty(),
        "SHACL declaration requires parameters"
    );
    let mut present = false;
    for parameter in parameters {
        let path = shapes.one(&parameter, &iri(&format!("{SH}path")))?;
        ensure!(path.starts_with('<'), "SHACL parameter path must be an IRI");
        let expanded = value(&path);
        let name = expanded.rsplit(['#', '/']).next().unwrap_or(&expanded);
        ensure!(
            ![
                "this",
                "value",
                "path",
                "PATH",
                "currentShape",
                "shapesGraph"
            ]
            .contains(&name),
            "reserved SHACL parameter name"
        );
        oxigraph::model::Variable::new(name)?;
        let values = shapes.objects(instance, &path);
        let optional = shapes
            .objects(&parameter, &iri(&format!("{SH}optional")))
            .iter()
            .any(|v| value(v) == "true");
        if values.is_empty() {
            if optional {
                continue;
            }
            ensure!(!required, "SHACL target parameter is missing");
            return Ok(vec![]);
        }
        present = true;
        let mut next = vec![];
        for bindings in &combinations {
            for term in &values {
                ensure!(next.len() < 4096, "SHACL parameter binding budget exceeded");
                let mut bindings = bindings.clone();
                ensure!(
                    bindings.insert(name.to_owned(), term.clone()).is_none(),
                    "duplicate SHACL parameter name"
                );
                next.push(bindings);
            }
        }
        combinations = next;
    }
    Ok(if present || required {
        combinations
    } else {
        vec![]
    })
}

fn rdf_list(shapes: &Graph, head: &str) -> Result<Vec<String>> {
    let mut node = head.to_owned();
    let mut result = vec![];
    let mut seen = BTreeSet::new();
    while node != iri(&format!("{RDF}nil")) {
        ensure!(
            seen.insert(node.clone()) && result.len() < 256,
            "invalid or oversized SHACL list"
        );
        result.push(shapes.one(&node, &iri(&format!("{RDF}first")))?);
        node = shapes.one(&node, &iri(&format!("{RDF}rest")))?;
    }
    Ok(result)
}
fn path_expression(shapes: &Graph, path: &str, depth: usize) -> Result<String> {
    ensure!(depth < 32, "SHACL path depth budget exceeded");
    if path.starts_with('<') {
        return Ok(path.into());
    }
    for (local, prefix, suffix) in [
        ("inversePath", "^(", ")"),
        ("zeroOrMorePath", "(", ")*"),
        ("oneOrMorePath", "(", ")+"),
        ("zeroOrOnePath", "(", ")?"),
    ] {
        if let Some(child) = shapes
            .objects(path, &iri(&format!("{SH}{local}")))
            .iter()
            .next()
        {
            return Ok(format!(
                "{}{}{}",
                prefix,
                path_expression(shapes, child, depth + 1)?,
                suffix
            ));
        }
    }
    if let Some(head) = shapes
        .objects(path, &iri(&format!("{SH}alternativePath")))
        .iter()
        .next()
    {
        return Ok(format!(
            "({})",
            rdf_list(shapes, head)?
                .iter()
                .map(|p| path_expression(shapes, p, depth + 1))
                .collect::<Result<Vec<_>>>()?
                .join("|")
        ));
    }
    Ok(format!(
        "({})",
        rdf_list(shapes, path)?
            .iter()
            .map(|p| path_expression(shapes, p, depth + 1))
            .collect::<Result<Vec<_>>>()?
            .join("/")
    ))
}

struct Composed<'a> {
    data: &'a Graph,
    queries: &'a PreparedQueries,
    shapes: &'a Graph,
    reachable: &'a BTreeSet<String>,
    native: Option<InMemoryGraph>,
    schemas: BTreeMap<String, (SchemaIR, Option<NativeEngine>)>,
    cache: BTreeMap<(String, String), Vec<Value>>,
    active: BTreeSet<(String, String)>,
    visits: usize,
    max_depth: usize,
    work_budget: usize,
}
impl Composed<'_> {
    fn evaluate(&mut self, shape: &str, focus: &str) -> Result<Vec<Value>> {
        let key = (shape.to_owned(), focus.to_owned());
        if let Some(result) = self.cache.get(&key) {
            return Ok(result.clone());
        }
        ensure!(
            self.active.insert(key.clone()),
            "recursive SHACL shape evaluation is unsupported"
        );
        ensure!(
            self.active.len() <= self.max_depth,
            "SHACL validation depth budget exceeded"
        );
        self.visits += 1;
        ensure!(
            self.visits <= self.work_budget,
            "SHACL evaluation budget exceeded"
        );
        let result = self.evaluate_inner(shape, focus)?;
        self.active.remove(&key);
        self.cache.insert(key, result.clone());
        Ok(result)
    }
    fn evaluate_inner(&mut self, shape: &str, focus: &str) -> Result<Vec<Value>> {
        if self
            .shapes
            .objects(shape, &iri(&format!("{SH}deactivated")))
            .iter()
            .any(|t| value(t) == "true")
        {
            return Ok(vec![]);
        }
        let path = self
            .shapes
            .objects(shape, &iri(&format!("{SH}path")))
            .iter()
            .next()
            .map(|p| path_expression(self.shapes, p, 0))
            .transpose()?;
        let values = if let Some(path) = &path {
            self.queries
                .select_focus(
                    &format!("SELECT ?this ?v WHERE {{ ?this {path} ?v }}"),
                    focus,
                )?
                .into_iter()
                .filter_map(|row| row.get("v").map(ToString::to_string))
                .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::from([focus.to_owned()])
        };
        let schema_key = shape.to_owned();
        if !self.schemas.contains_key(&schema_key) {
            let delegated = [
                "property",
                "hasValue",
                "node",
                "not",
                "or",
                "and",
                "xone",
                "sparql",
                "qualifiedValueShape",
                "qualifiedMinCount",
                "qualifiedMaxCount",
                "qualifiedValueShapesDisjoint",
            ];
            let targets = [
                "targetClass",
                "targetNode",
                "targetObjectsOf",
                "targetSubjectsOf",
                "target",
            ];
            let mut graph = Graph {
                triples: self
                    .shapes
                    .triples
                    .iter()
                    .filter(|t| {
                        self.reachable.contains(&t[0])
                            && !targets.iter().any(|p| t[1] == iri(&format!("{SH}{p}")))
                            && !delegated.iter().any(|p| t[1] == iri(&format!("{SH}{p}")))
                    })
                    .cloned()
                    .collect(),
            };
            graph.triples.insert([
                shape.into(),
                iri(&format!("{RDF}type")),
                iri(&format!(
                    "{SH}{}",
                    if path.is_some() {
                        "PropertyShape"
                    } else {
                        "NodeShape"
                    }
                )),
            ]);
            // Closed constraints need the allowed property paths, without their delegated constraints.
            if self
                .shapes
                .objects(shape, &iri(&format!("{SH}closed")))
                .iter()
                .any(|v| value(v) == "true")
            {
                for child in self.shapes.objects(shape, &iri(&format!("{SH}property"))) {
                    graph
                        .triples
                        .retain(|t| t[0] != child || t[1] == iri(&format!("{SH}path")));
                    graph
                        .triples
                        .insert([shape.into(), iri(&format!("{SH}property")), child]);
                }
            }
            let schema = SchemaIR::from_str(
                &graph.nt(),
                &RDFFormat::NTriples,
                None,
                &ReaderMode::default(),
            )
            .map_err(|e| anyhow::anyhow!("SHACL compilation failed: {e}"))?;
            // Cache entries use schema-local indices, so each schema owns its runner.
            self.schemas.insert(schema_key.clone(), (schema, None));
        }
        let (schema, runner) = self.schemas.get_mut(&schema_key).expect("prepared shape");
        let compiled = schema
            .iter()
            .find(|(id, _)| id.to_string() == shape || id.to_string() == value(shape))
            .map(|(_, shape)| shape)
            .ok_or_else(|| anyhow::anyhow!("compiled SHACL shape unavailable"))?;
        let mut violations = Vec::new();
        // Delegated-only shapes do not need a second representation of the data graph.
        if compiled.closed()
            || !compiled.components().is_empty()
            || !compiled.property_shapes().is_empty()
        {
            if self.native.is_none() {
                self.native = Some(native_graph(self.data)?);
            }
            let native = self.native.as_ref().expect("prepared native graph");
            if runner.is_none() {
                let mut engine = NativeEngine::new();
                engine
                    .build_indexes(native)
                    .map_err(|e| anyhow::anyhow!("SHACL index construction failed: {e}"))?;
                *runner = Some(engine);
            }
            let targets = FocusNodes::single(focus.parse::<oxrdf::Term>()?);
            let report = compiled
                .validate(
                    native,
                    runner.as_mut().expect("prepared engine"),
                    Some(&targets),
                    Some(compiled),
                    schema,
                )
                .map_err(|e| anyhow::anyhow!("SHACL execution failed: {e}"))?;
            violations.extend(report.iter().map(|r|json!({"focus_node":r.focus_node().to_string(),"path":r.path().map(ToString::to_string).unwrap_or_default(),"message":r.message().unwrap_or("SHACL violation"),"shape_ref":r.source().map(ToString::to_string).unwrap_or_default()})));
        }
        let message = self
            .shapes
            .objects(shape, &iri(&format!("{SH}message")))
            .iter()
            .next()
            .map(|s| value(s))
            .unwrap_or_else(|| "SHACL nested constraint violation".into());
        let violation = |component: &str| json!({"focus_node":value(focus),"path":path,"message":message,"source_shape":value(shape),"source_constraint_component":format!("{SH}{component}"),"shape_ref":value(shape)});
        for required in self.shapes.objects(shape, &iri(&format!("{SH}hasValue"))) {
            if !values.contains(&required) {
                violations.push(violation("HasValueConstraintComponent"));
            }
        }
        for constraint in self.shapes.objects(shape, &iri(&format!("{SH}sparql"))) {
            if self
                .shapes
                .objects(&constraint, &iri(&format!("{SH}deactivated")))
                .iter()
                .any(|t| value(t) == "true")
            {
                continue;
            }
            let query = value(&self.shapes.one(&constraint, &iri(&format!("{SH}select")))?);
            let query = prefixed_query(self.shapes, &constraint, &query)?;
            let query = crate::query::shacl_path(&query, path.as_deref())?;
            let rows = self.queries.select_focus(&query, focus)?;
            for _row in rows {
                violations.push(violation("SPARQLConstraintComponent"));
            }
        }
        for component in self.shapes.subjects(
            &iri(&format!("{RDF}type")),
            &iri(&format!("{SH}ConstraintComponent")),
        ) {
            for mut bindings in parameter_bindings(self.shapes, &component, shape, false)? {
                bindings.insert("this".into(), focus.into());
                bindings.insert("currentShape".into(), shape.into());
                let validators = self
                    .shapes
                    .objects(&component, &iri(&format!("{SH}validator")));
                ensure!(
                    !validators.is_empty(),
                    "SHACL custom constraint validator is missing"
                );
                for validator in &validators {
                    let ask = self.shapes.objects(validator, &iri(&format!("{SH}ask")));
                    let select = self.shapes.objects(validator, &iri(&format!("{SH}select")));
                    ensure!(
                        ask.len() + select.len() == 1,
                        "SHACL validator must declare one ASK or SELECT"
                    );
                    let text = value(ask.iter().chain(select.iter()).next().expect("one query"));
                    let text = prefixed_query(self.shapes, validator, &text)?;
                    let text = crate::query::shacl_path(&text, path.as_deref())?;
                    for node in &values {
                        bindings.insert("value".into(), node.clone());
                        let (boolean, rows) = self.queries.query_bindings(&text, &bindings)?;
                        let failed = if !ask.is_empty() {
                            !boolean.ok_or_else(|| {
                                anyhow::anyhow!("SHACL ASK validator requires ASK")
                            })?
                        } else {
                            ensure!(boolean.is_none(), "SHACL SELECT validator requires SELECT");
                            !rows.is_empty()
                        };
                        if failed {
                            let mut report = violation("SPARQLConstraintComponent");
                            report["source_constraint_component"] = json!(value(&component));
                            report["value"] = json!(value(node));
                            violations.push(report);
                        }
                    }
                }
            }
        }
        for child in self.shapes.objects(shape, &iri(&format!("{SH}property"))) {
            for node in &values {
                violations.extend(self.evaluate(&child, node)?);
            }
        }
        for child in self.shapes.objects(shape, &iri(&format!("{SH}node"))) {
            for node in &values {
                if !self.evaluate(&child, node)?.is_empty() {
                    violations.push(violation("NodeConstraintComponent"));
                }
            }
        }
        for child in self.shapes.objects(shape, &iri(&format!("{SH}not"))) {
            for node in &values {
                if self.evaluate(&child, node)?.is_empty() {
                    violations.push(violation("NotConstraintComponent"));
                }
            }
        }
        for (local, component) in [
            ("or", "OrConstraintComponent"),
            ("and", "AndConstraintComponent"),
            ("xone", "XoneConstraintComponent"),
        ] {
            for head in self.shapes.objects(shape, &iri(&format!("{SH}{local}"))) {
                let children = rdf_list(self.shapes, &head)?;
                for node in &values {
                    let mut passed = 0;
                    for child in &children {
                        if self.evaluate(child, node)?.is_empty() {
                            passed += 1;
                        }
                    }
                    if match local {
                        "or" => passed == 0,
                        "and" => passed != children.len(),
                        _ => passed != 1,
                    } {
                        violations.push(violation(component));
                    }
                }
            }
        }
        for child in self
            .shapes
            .objects(shape, &iri(&format!("{SH}qualifiedValueShape")))
        {
            let mut siblings = BTreeSet::new();
            if self
                .shapes
                .objects(shape, &iri(&format!("{SH}qualifiedValueShapesDisjoint")))
                .iter()
                .any(|t| value(t) == "true")
            {
                for parent in self.shapes.subjects(&iri(&format!("{SH}property")), shape) {
                    for property in self.shapes.objects(&parent, &iri(&format!("{SH}property"))) {
                        if property != shape {
                            siblings.extend(
                                self.shapes
                                    .objects(&property, &iri(&format!("{SH}qualifiedValueShape"))),
                            );
                        }
                    }
                }
            }
            let mut count = 0;
            for node in &values {
                if self.evaluate(&child, node)?.is_empty() {
                    let mut shared = false;
                    for sibling in &siblings {
                        if self.evaluate(sibling, node)?.is_empty() {
                            shared = true;
                            break;
                        }
                    }
                    if !shared {
                        count += 1u64;
                    }
                }
            }
            for (local, component) in [
                ("qualifiedMinCount", "QualifiedMinCountConstraintComponent"),
                ("qualifiedMaxCount", "QualifiedMaxCountConstraintComponent"),
            ] {
                for bound in self.shapes.objects(shape, &iri(&format!("{SH}{local}"))) {
                    let bound = value(&bound).parse::<u64>()?;
                    if (local == "qualifiedMinCount" && count < bound)
                        || (local == "qualifiedMaxCount" && count > bound)
                    {
                        violations.push(violation(component));
                    }
                }
            }
        }
        ensure!(
            violations.len() <= self.work_budget,
            "SHACL report budget exceeded"
        );
        Ok(violations)
    }
}
fn native_graph(data: &Graph) -> Result<rudof_rdf::rdf_impl::InMemoryGraph> {
    let triples = &data.triples;
    let mut terms = HashMap::new();
    for term in triples.iter().flatten() {
        if !terms.contains_key(term.as_str()) {
            let parsed = if let Some(label) = term.strip_prefix("_:") {
                // 独立 Term 解析会截断内部连续点；构造器允许冒号，须保持 NTriples 边界。
                ensure!(
                    !label.contains(':'),
                    "SHACL input failed: invalid blank node label"
                );
                oxrdf::BlankNode::new(label)
                    .map(oxrdf::Term::BlankNode)
                    .map_err(|e| anyhow::anyhow!("SHACL input failed: {e}"))?
            } else {
                term.parse::<oxrdf::Term>()
                    .map_err(|e| anyhow::anyhow!("SHACL input failed: {e}"))?
            };
            terms.insert(term.as_str(), parsed);
        }
    }
    let mut graph = rudof_rdf::rdf_impl::InMemoryGraph::new();
    for [s, p, o] in triples {
        let subject: oxrdf::NamedOrBlankNodeRef<'_> = match &terms[s.as_str()] {
            oxrdf::Term::NamedNode(node) => node.as_ref().into(),
            oxrdf::Term::BlankNode(node) => node.as_ref().into(),
            _ => anyhow::bail!("SHACL input failed: subject must be an IRI or blank node"),
        };
        let predicate = match &terms[p.as_str()] {
            oxrdf::Term::NamedNode(node) => node.as_ref(),
            _ => anyhow::bail!("SHACL input failed: predicate must be an IRI"),
        };
        graph.add_triple_ref(subject, predicate, terms[o.as_str()].as_ref())?;
    }
    Ok(graph)
}

fn validate_composed(
    data: &Graph,
    queries: &PreparedQueries,
    shapes: &Graph,
    reachable: &BTreeSet<String>,
    foci: &BTreeMap<String, BTreeSet<String>>,
    max_depth: usize,
    work_budget: usize,
) -> Result<Value> {
    let mut context = Composed {
        data,
        queries,
        shapes,
        reachable,
        native: None,
        schemas: BTreeMap::new(),
        cache: BTreeMap::new(),
        active: BTreeSet::new(),
        visits: 0,
        max_depth,
        work_budget,
    };
    let mut violations = vec![];
    for (shape, nodes) in foci {
        for node in nodes {
            violations.extend(context.evaluate(shape, node)?);
        }
    }
    violations.sort_by_key(Value::to_string);
    violations.dedup();
    Ok(
        json!({"conforms":violations.is_empty(),"violations":violations,"engine":{"name":"ontokiln-rust-shacl","version":env!("CARGO_PKG_VERSION")}}),
    )
}
