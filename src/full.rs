//! All parser, dictionary insertion, sorting, joins and fixpoint work runs here.
//! SQLite's native sorter/BTrees are disk-backed; no RDF row calls into Python.
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term};
use oxttl::{NQuadsParser, NTriplesParser, TriGParser, TurtleParser};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::str::FromStr;
use std::time::Instant;

type Result<T> = std::result::Result<T, String>;
fn err(code: &str, message: impl std::fmt::Display) -> String {
    json!({"code":code,"message":message.to_string()}).to_string()
}
fn sql<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|e| {
        let limited=matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if matches!(x.code,rusqlite::ErrorCode::DiskFull|rusqlite::ErrorCode::OperationInterrupted));
        err(if limited{"RESOURCE_LIMIT_EXCEEDED"}else{"INDEX_BUILD_FAILED"},e)
    })
}
fn setup(path: &str, budget: u64, disk: u64) -> Result<Connection> {
    let c = sql(Connection::open(path))?;
    // Leave memory for parser, term cache and the process itself. Main and temp
    // schemas each receive 1/8 of the RSS allowance. Index sort uses FILE spill.
    sql(c.execute_batch(&format!(
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
        PRAGMA temp_store=FILE; PRAGMA cache_size=-{}; PRAGMA temp.cache_size=-{};
        PRAGMA max_page_count={}; PRAGMA threads=1;",
        budget / 8192,
        budget / 8192,
        disk / 4096
    )))?;
    Ok(c)
}

#[derive(Deserialize)]
struct Source {
    path: String,
    document_id: String,
    format: String,
    base_iri: Option<String>,
    #[serde(default)]
    is_rule: bool,
    #[serde(default)]
    unchecked_turtle_terms: bool,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    op: String,
    var: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    rule_id: String,
    head: Vec<String>,
    body: Vec<Vec<String>>,
    #[serde(default)]
    filters: Vec<Filter>,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    schema_version: String,
    rules: Vec<Rule>,
    sccs: Vec<Vec<usize>>,
}

fn atom(a: &[String]) -> Vec<String> {
    if a.len() == 3 {
        std::iter::once("DEFAULT".to_owned())
            .chain(a.iter().cloned())
            .collect()
    } else {
        a.to_vec()
    }
}
fn variable(v: &str) -> bool {
    v.starts_with('?')
        && v.len() > 1
        && v[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}
fn validate_rules(rules: &mut [Rule]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for r in rules {
        if r.rule_id.is_empty()
            || !ids.insert(r.rule_id.clone())
            || r.body.is_empty()
            || r.body.len() > 16
        {
            return Err(err(
                "RULE_INVALID",
                "rule_id must be unique and body must have 1..16 atoms",
            ));
        }
        r.head = atom(&r.head);
        r.body = r.body.iter().map(|a| atom(a)).collect();
        if r.head.len() != 4 || r.body.iter().any(|a| a.len() != 4) {
            return Err(err("RULE_INVALID", "atoms need 3 or 4 terms"));
        }
        let bound: BTreeSet<_> = r
            .body
            .iter()
            .flatten()
            .filter(|v| variable(v))
            .cloned()
            .collect();
        for v in r.head.iter().filter(|v| variable(v)) {
            if !bound.contains(v) {
                return Err(err(
                    "UNSAFE_RULE",
                    format!("{} unbound head {v}", r.rule_id),
                ));
            }
        }
        for f in &r.filters {
            if !bound.contains(&f.var)
                || !["isIRI", "isResource", "isLiteral"].contains(&f.op.as_str())
            {
                return Err(err(
                    "UNSUPPORTED_RULE_PROFILE",
                    "only bound RDF term-kind filters are supported",
                ));
            }
        }
        for (i, v) in r.head.iter().enumerate() {
            if v.starts_with('?') && !variable(v) {
                return Err(err("RULE_INVALID", "invalid variable"));
            }
            if !variable(v) && v != "DEFAULT" {
                let t = Term::from_str(v).map_err(|e| err("RULE_INVALID", e))?;
                if (i == 0 || i == 1) && matches!(t, Term::Literal(_))
                    || i == 2 && !matches!(t, Term::NamedNode(_))
                {
                    return Err(err("RULE_INVALID", "head is not a valid RDF Quad"));
                }
            }
        }
        for a in r.body.iter_mut().chain(std::iter::once(&mut r.head)) {
            for (i, v) in a.iter_mut().enumerate() {
                if v == "DEFAULT" && i != 0 {
                    return Err(err(
                        "RULE_INVALID",
                        "DEFAULT is only valid in graph position",
                    ));
                }
                if !variable(v) && v != "DEFAULT" {
                    let t = Term::from_str(v).map_err(|e| err("RULE_INVALID", e))?;
                    *v = match t {
                        Term::Literal(l) if l.language().is_some() => {
                            oxrdf::Literal::new_language_tagged_literal(
                                l.value(),
                                l.language().unwrap().to_ascii_lowercase(),
                            )
                            .map_err(|e| err("RULE_INVALID", e))?
                            .to_string()
                        }
                        _ => t.to_string(),
                    };
                }
            }
        }
    }
    Ok(())
}

// Deterministic predicate dependency SCCs. Wildcard predicates conservatively
// depend on every producer. Topological SCC execution preserves seed ordering.
fn sccs(rules: &[Rule]) -> Vec<Vec<usize>> {
    let n = rules.len();
    let mut reach = vec![vec![false; n]; n];
    for i in 0..n {
        for j in 0..n {
            let hp = &rules[i].head[2];
            reach[i][j] = rules[j]
                .body
                .iter()
                .any(|a| variable(hp) || variable(&a[2]) || hp == &a[2]);
        }
    }
    for k in 0..n {
        for i in 0..n {
            for j in 0..n {
                reach[i][j] |= reach[i][k] && reach[k][j];
            }
        }
    }
    let mut seen = BTreeSet::<usize>::new();
    let mut groups = Vec::new();
    for (i, _) in reach.iter().enumerate().take(n) {
        if seen.contains(&i) {
            continue;
        }
        let g: Vec<_> = (i..n)
            .filter(|&j| i == j || reach[i][j] && reach[j][i])
            .collect();
        seen.extend(g.iter().copied());
        groups.push(g);
    }
    let mut result = Vec::new();
    let mut remaining: BTreeSet<_> = (0..groups.len()).collect();
    while !remaining.is_empty() {
        let next = *remaining
            .iter()
            .find(|&&g| {
                !remaining.iter().any(|&h| {
                    h != g
                        && groups[h]
                            .iter()
                            .any(|&i| groups[g].iter().any(|&j| reach[i][j]))
                })
            })
            .unwrap();
        remaining.remove(&next);
        result.push(groups[next].clone());
    }
    result
}

struct Dictionary<'a> {
    c: &'a Connection,
    cache: HashMap<String, i64>,
    cache_bytes: usize,
    cache_limit: usize,
    max_term: usize,
    doc: String,
    document: i64,
}
impl Dictionary<'_> {
    fn term(&mut self, t: Term) -> Result<i64> {
        let (name, kind, value, datatype, lang) = match t {
            Term::NamedNode(n) => (
                n.to_string(),
                "I",
                n.as_str().to_owned(),
                String::new(),
                String::new(),
            ),
            Term::BlankNode(b) => {
                let k = b.as_str();
                sql(sql(self
                    .c
                    .prepare_cached("INSERT OR IGNORE INTO bnodes(raw) VALUES(?)"))?
                .execute([k]))?;
                let id: i64 =
                    sql(self
                        .c
                        .query_row("SELECT rowid FROM bnodes WHERE raw=?", [k], |r| r.get(0)))?;
                let n = format!("_:d{}n{id}", self.doc);
                (
                    n.clone(),
                    "B",
                    n[2..].to_owned(),
                    String::new(),
                    String::new(),
                )
            }
            Term::Triple(_) => {
                return Err(err(
                    "TTL_PARSE_ERROR",
                    "quoted RDF triples are outside the full-source contract",
                ));
            }
            Term::Literal(l) => {
                // oxrdf's Literal preserves lexical forms; never store a value-
                // normalized native Store literal as the canonical RDF identity.
                let lang = l.language().unwrap_or("").to_ascii_lowercase();
                let literal = if lang.is_empty() {
                    l
                } else {
                    oxrdf::Literal::new_language_tagged_literal(l.value(), &lang)
                        .map_err(|e| err("TTL_PARSE_ERROR", e))?
                };
                (
                    literal.to_string(),
                    "L",
                    literal.value().to_owned(),
                    literal.datatype().as_str().to_owned(),
                    lang,
                )
            }
        };
        if name.len() > self.max_term {
            return Err(err(
                "RESOURCE_LIMIT_EXCEEDED",
                "term exceeds max_term_bytes",
            ));
        }
        if let Some(&id) = self.cache.get(&name) {
            return Ok(id);
        }
        let mut stmt = sql(self.c.prepare_cached("SELECT id FROM terms WHERE n=?"))?;
        let id = match sql(stmt.query_row([&name], |r| r.get(0)).optional())? {
            Some(id) => id,
            None => {
                sql(sql(self.c.prepare_cached(
                    "INSERT INTO terms(n,kind,value,datatype,lang) VALUES(?,?,?,?,?)",
                ))?
                .execute(params![name, kind, value, datatype, lang]))?;
                self.c.last_insert_rowid()
            }
        };
        if self.cache_bytes + name.len() + 64 > self.cache_limit {
            self.cache.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += name.len() + 64;
        self.cache.insert(name, id);
        Ok(id)
    }
    fn constant(&mut self, s: &str) -> Result<i64> {
        if s == "DEFAULT" {
            return Ok(0);
        }
        let t = Term::from_str(s).map_err(|e| err("RULE_INVALID", e))?;
        if matches!(t, Term::BlankNode(_)) {
            return Err(err(
                "RULE_INVALID",
                "rule constants cannot name document Blank Nodes",
            ));
        }
        self.term(t)
    }
    fn quad(&mut self, q: Quad) -> Result<()> {
        let g = match q.graph_name {
            GraphName::DefaultGraph => 0,
            GraphName::NamedNode(n) => self.term(n.into())?,
            GraphName::BlankNode(b) => self.term(b.into())?,
        };
        let s = self.term(match q.subject {
            NamedOrBlankNode::NamedNode(n) => n.into(),
            NamedOrBlankNode::BlankNode(b) => b.into(),
        })?;
        let p = self.term(q.predicate.into())?;
        let o = self.term(q.object)?;
        sql(
            sql(self.c.prepare_cached("INSERT INTO raw VALUES(?,?,?,?,?)"))?.execute(params![
                g,
                s,
                p,
                o,
                self.document
            ]),
        )?;
        Ok(())
    }
}

fn property_chain_rules(c: &Connection) -> Result<Vec<Rule>> {
    let mut generated = BTreeMap::<String, Rule>::new();
    let mut stmt=sql(c.prepare("SELECT f.g,s.n,f.o FROM facts f JOIN terms p ON f.p=p.id JOIN terms s ON s.id=f.s WHERE p.value='http://www.w3.org/2002/07/owl#propertyChainAxiom' ORDER BY s.n,f.o"))?;
    let rows = sql(stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    }))?;
    for row in rows {
        let (graph, predicate, mut cursor) = sql(row)?;
        if graph != 0 {
            return Err(err(
                "UNSUPPORTED_RULE_PROFILE",
                "property-chain axioms require default graph in this profile",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut chain = Vec::new();
        loop {
            let (name, kind): (String, String) = sql(c.query_row(
                "SELECT n,kind FROM terms WHERE id=?",
                [cursor],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ))?;
            if name == "<http://www.w3.org/1999/02/22-rdf-syntax-ns#nil>" {
                break;
            }
            if kind != "B" || !seen.insert(cursor) || chain.len() >= 16 {
                return Err(err(
                    "RULE_INVALID",
                    "property-chain RDF list is cyclic or exceeds 16 atoms",
                ));
            }
            let get = |pred: &str| -> Result<Vec<i64>> {
                let mut st=sql(c.prepare("SELECT f.o FROM facts f JOIN terms p ON p.id=f.p WHERE f.g=0 AND f.s=? AND p.value=?"))?;
                sql(st.query_map(params![cursor, pred], |r| r.get(0)))?
                    .map(sql)
                    .collect()
            };
            let first = get("http://www.w3.org/1999/02/22-rdf-syntax-ns#first")?;
            let rest = get("http://www.w3.org/1999/02/22-rdf-syntax-ns#rest")?;
            if first.len() != 1 || rest.len() != 1 {
                return Err(err(
                    "RULE_INVALID",
                    "property-chain list requires exactly one first/rest per node",
                ));
            }
            let (mut n, k): (String, String) = sql(c.query_row(
                "SELECT n,kind FROM terms WHERE id=?",
                [first[0]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ))?;
            let inverse = k == "B";
            if inverse {
                let mut st = sql(c.prepare("SELECT o.n,o.kind FROM facts f JOIN terms p ON p.id=f.p JOIN terms o ON o.id=f.o WHERE f.g=0 AND f.s=? AND p.value='http://www.w3.org/2002/07/owl#inverseOf' LIMIT 2"))?;
                let targets: Vec<(String, String)> =
                    sql(st.query_map([first[0]], |r| Ok((r.get(0)?, r.get(1)?))))?
                        .map(sql)
                        .collect::<Result<_>>()?;
                if targets.len() != 1 || targets[0].1 != "I" {
                    return Err(err(
                        "UNSUPPORTED_RULE_PROFILE",
                        "inverse property-chain member requires exactly one default-graph inverseOf named property",
                    ));
                }
                n = targets[0].0.clone();
            } else if k != "I" {
                return Err(err(
                    "UNSUPPORTED_RULE_PROFILE",
                    "property-chain members must be named properties or inverse property expressions",
                ));
            }
            chain.push((n, inverse));
            cursor = rest[0];
        }
        if chain.is_empty() {
            return Err(err("RULE_INVALID", "empty property chain"));
        }
        let body = chain
            .iter()
            .enumerate()
            .map(|(i, (p, inverse))| {
                let (s, o) = if *inverse { (i + 1, i) } else { (i, i + 1) };
                vec![format!("?v{s}"), p.clone(), format!("?v{o}")]
            })
            .collect();
        let filters = chain
            .iter()
            .enumerate()
            .filter(|(_, (_, inverse))| *inverse)
            .map(|(i, _)| Filter {
                op: "isResource".into(),
                var: format!("?v{i}"),
            })
            .collect();
        let name = format!(
            "owl-chain:{:x}",
            Sha256::digest(serde_json::to_vec(&(predicate.clone(), chain.clone())).unwrap())
        );
        generated.insert(
            name.clone(),
            Rule {
                rule_id: name,
                head: vec!["?v0".into(), predicate, format!("?v{}", chain.len())],
                body,
                filters,
            },
        );
    }
    Ok(generated.into_values().collect())
}

fn optional_id(c: &Connection, key: &str) -> Result<Option<i64>> {
    sql(c
        .query_row("SELECT id FROM terms WHERE n=?", [key], |r| r.get(0))
        .optional())
}

fn resolve_imports(c: &Connection, request: &Value) -> Result<Value> {
    let automatic = request["import_lock"].is_null();
    let mut locks = if automatic {
        serde_json::Map::new()
    } else {
        request["import_lock"]
            .as_object()
            .ok_or_else(|| err("SOURCE_INVALID", "explicit import_lock must be an object"))?
            .clone()
    };
    if !automatic {
        for (iri, binding) in &locks {
            oxrdf::NamedNode::new(iri).map_err(|e| err("SOURCE_INVALID", e))?;
            let docs: Vec<&str> = match binding {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a
                    .iter()
                    .map(|x| {
                        x.as_str().ok_or_else(|| {
                            err("SOURCE_INVALID", "import document IDs must be strings")
                        })
                    })
                    .collect::<Result<_>>()?,
                _ => {
                    return Err(err(
                        "SOURCE_INVALID",
                        "import binding must name one or more documents",
                    ));
                }
            };
            if docs.is_empty() {
                return Err(err("SOURCE_INVALID", "empty import document binding"));
            }
            for doc in docs {
                let exists: i64 = sql(c.query_row(
                    "SELECT COUNT(*) FROM source_documents WHERE document_id=?",
                    [doc],
                    |r| r.get(0),
                ))?;
                if exists != 1 {
                    return Err(err(
                        "SOURCE_INVALID",
                        format!("import lock names missing document: {doc}"),
                    ));
                }
            }
        }
    }
    let Some(imports) = optional_id(c, "<http://www.w3.org/2002/07/owl#imports>")? else {
        return Ok(Value::Object(locks));
    };
    let rdf_type = optional_id(c, "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>")?;
    let ontology = optional_id(c, "<http://www.w3.org/2002/07/owl#Ontology>")?;
    let mut stmt=sql(c.prepare("SELECT DISTINCT o.id,o.kind,o.value FROM facts f JOIN terms o ON o.id=f.o WHERE f.p=? LIMIT 100001"))?;
    let rows = sql(stmt.query_map([imports], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    }))?;
    let mut count = 0usize;
    let mut bindings = 0usize;
    for row in rows {
        let (subject, kind, iri) = sql(row)?;
        count += 1;
        if count > 100000 {
            return Err(err(
                "RESOURCE_LIMIT_EXCEEDED",
                "import manifest exceeds 100000 modules",
            ));
        }
        if kind != "I" {
            return Err(err("SOURCE_INVALID", "owl:imports must reference an IRI"));
        }
        if !automatic {
            if !locks.contains_key(&iri) {
                return Err(err(
                    "SOURCE_INVALID",
                    format!("missing frozen import: {iri}"),
                ));
            }
            continue;
        }
        let (Some(rdf_type), Some(ontology)) = (rdf_type, ontology) else {
            return Err(err(
                "SOURCE_INVALID",
                format!("imported ontology has no frozen declaration: {iri}"),
            ));
        };
        let mut declared=sql(c.prepare("SELECT DISTINCT d.document_id FROM source_facts f JOIN source_documents d ON d.id=f.document WHERE f.s=? AND f.p=? AND f.o=? ORDER BY d.document_id"))?;
        let docs = sql(
            declared.query_map(params![subject, rdf_type, ontology], |r| {
                r.get::<_, String>(0)
            }),
        )?
        .map(sql)
        .collect::<Result<Vec<_>>>()?;
        if docs.is_empty() {
            return Err(err(
                "SOURCE_INVALID",
                format!("imported ontology has no frozen declaration: {iri}"),
            ));
        }
        bindings += docs.len();
        if bindings > 1000000 {
            return Err(err(
                "RESOURCE_LIMIT_EXCEEDED",
                "import manifest exceeds 1000000 document bindings",
            ));
        }
        locks.insert(iri, json!(docs));
    }
    let value = Value::Object(locks);
    if serde_json::to_vec(&value)
        .map_err(|e| err("SOURCE_INVALID", e))?
        .len()
        > 64 * 1024 * 1024
    {
        return Err(err(
            "RESOURCE_LIMIT_EXCEEDED",
            "resolved import manifest exceeds 64MiB",
        ));
    }
    Ok(value)
}

fn compile(v: Value) -> Result<Value> {
    let start = Instant::now();
    let database = v["database"]
        .as_str()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: database"))?;
    let memory_budget = v["memory_budget_bytes"].as_u64().ok_or_else(|| {
        err(
            "SOURCE_INVALID",
            "missing/invalid field: memory_budget_bytes",
        )
    })?;
    let disk_budget = v["disk_budget_bytes"]
        .as_u64()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: disk_budget_bytes"))?;
    let max_term = v["max_term_bytes"]
        .as_u64()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: max_term_bytes"))?;
    let sources: Vec<Source> =
        serde_json::from_value(v["sources"].clone()).map_err(|e| err("SOURCE_INVALID", e))?;
    let parser_options: Vec<Value> = sources.iter().map(|source| json!({"document_id":source.document_id,"unchecked_turtle_terms":source.unchecked_turtle_terms})).collect();
    let mut rules: Vec<Rule> =
        serde_json::from_value(v["rules"].clone()).map_err(|e| err("RULE_INVALID", e))?;
    if rules.len() > 256 {
        return Err(err(
            "RESOURCE_LIMIT_EXCEEDED",
            "at most 256 rules in native-v1",
        ));
    }
    validate_rules(&mut rules)?;
    let c = setup(database, memory_budget, disk_budget)?;
    sql(c.execute_batch("BEGIN; CREATE TABLE terms(id INTEGER PRIMARY KEY,n TEXT NOT NULL UNIQUE,kind TEXT NOT NULL,value TEXT NOT NULL,datatype TEXT NOT NULL,lang TEXT NOT NULL);
        CREATE TABLE bnodes(raw TEXT UNIQUE); CREATE TABLE source_documents(id INTEGER PRIMARY KEY,document_id TEXT NOT NULL UNIQUE);
        CREATE TABLE raw(g INTEGER,s INTEGER,p INTEGER,o INTEGER,document INTEGER);"))?;
    let mut dict = Dictionary {
        c: &c,
        cache: HashMap::new(),
        cache_bytes: 0,
        cache_limit: (memory_budget / 16) as usize,
        max_term: max_term as usize,
        doc: String::new(),
        document: 0,
    };
    let mut parsed = 0u64;
    for src in sources {
        sql(c.execute("DELETE FROM bnodes", []))?;
        dict.doc = format!("{:x}", Sha256::digest(src.document_id.as_bytes()));
        sql(c.execute(
            "INSERT INTO source_documents(document_id) VALUES(?)",
            [&src.document_id],
        ))?;
        dict.document = c.last_insert_rowid();
        let f = BufReader::with_capacity(
            1 << 20,
            File::open(&src.path).map_err(|e| err("SOURCE_INVALID", e))?,
        );
        let input: Box<dyn Iterator<Item = std::result::Result<Quad, oxttl::TurtleParseError>>> =
            match src.format.as_str() {
                "turtle" => {
                    let mut p = TurtleParser::new();
                    if src.unchecked_turtle_terms {
                        p = p.lenient();
                    }
                    if let Some(base) = src.base_iri {
                        p = p
                            .with_base_iri(base)
                            .map_err(|e| err("SOURCE_INVALID", e))?;
                    }
                    Box::new(p.for_reader(f).map(|r| {
                        r.map(|t| {
                            Quad::new(t.subject, t.predicate, t.object, GraphName::DefaultGraph)
                        })
                    }))
                }
                "ntriples" => Box::new(NTriplesParser::new().for_reader(f).map(|r| {
                    r.map(|t| Quad::new(t.subject, t.predicate, t.object, GraphName::DefaultGraph))
                })),
                "nquads" => Box::new(NQuadsParser::new().for_reader(f)),
                "trig" => {
                    let mut p = TriGParser::new();
                    if let Some(base) = src.base_iri {
                        p = p
                            .with_base_iri(base)
                            .map_err(|e| err("SOURCE_INVALID", e))?;
                    }
                    Box::new(p.for_reader(f))
                }
                _ => {
                    return Err(err(
                        "SOURCE_INVALID",
                        "format must be turtle/ntriples/nquads/trig",
                    ));
                }
            };
        for q in input {
            let q = q.map_err(|e| err("TTL_PARSE_ERROR", format!("{}: {e}", src.document_id)))?;
            if src.is_rule {
                let p = q.predicate.as_str();
                if p.starts_with("http://www.w3.org/2002/07/owl#")
                    && ![
                        "http://www.w3.org/2002/07/owl#imports",
                        "http://www.w3.org/2002/07/owl#propertyChainAxiom",
                        "http://www.w3.org/2002/07/owl#inverseOf",
                    ]
                    .contains(&p)
                {
                    return Err(err(
                        "UNSUPPORTED_RULE_PROFILE",
                        format!("unsupported executable OWL axiom: {p}"),
                    ));
                }
                if p == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
                    && let Term::NamedNode(rule_type) = &q.object
                {
                    let iri = rule_type.as_str();
                    // Ordinary declarations are explicit metadata; only
                    // SymmetricProperty has executable type semantics here.
                    if iri.starts_with("http://www.w3.org/2002/07/owl#")
                        && ![
                            "http://www.w3.org/2002/07/owl#Ontology",
                            "http://www.w3.org/2002/07/owl#Class",
                            "http://www.w3.org/2002/07/owl#ObjectProperty",
                            "http://www.w3.org/2002/07/owl#DatatypeProperty",
                            "http://www.w3.org/2002/07/owl#AnnotationProperty",
                            "http://www.w3.org/2002/07/owl#NamedIndividual",
                            "http://www.w3.org/2002/07/owl#SymmetricProperty",
                        ]
                        .contains(&iri)
                    {
                        return Err(err(
                            "UNSUPPORTED_RULE_PROFILE",
                            format!("unsupported executable OWL type: {iri}"),
                        ));
                    }
                }
            }
            dict.quad(q)?;
            parsed += 1;
        }
    }
    for r in &rules {
        for val in r.body.iter().flatten().chain(&r.head) {
            if !variable(val) {
                dict.constant(val)?;
            }
        }
    }
    drop(dict);
    let parse_seconds = start.elapsed().as_secs_f64();
    sql(c.execute_batch("CREATE TABLE facts(g INTEGER NOT NULL,s INTEGER NOT NULL,p INTEGER NOT NULL,o INTEGER NOT NULL,explicit INTEGER NOT NULL,PRIMARY KEY(g,s,p,o)) WITHOUT ROWID;
        INSERT INTO facts SELECT DISTINCT g,s,p,o,1 FROM raw ORDER BY g,s,p,o;
        CREATE TABLE source_facts(g INTEGER NOT NULL,s INTEGER NOT NULL,p INTEGER NOT NULL,o INTEGER NOT NULL,document INTEGER NOT NULL,PRIMARY KEY(g,s,p,o,document)) WITHOUT ROWID;
        INSERT INTO source_facts SELECT DISTINCT g,s,p,o,document FROM raw ORDER BY g,s,p,o,document;
        CREATE INDEX source_facts_spo ON source_facts(s,p,o,document,g);
        DROP TABLE raw; DROP TABLE bnodes;
        CREATE INDEX facts_pos ON facts(p,o,s,g); CREATE INDEX facts_osp ON facts(o,s,p,g);
        ANALYZE; COMMIT; VACUUM;"))?;
    let count: i64 = sql(c.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)))?;
    let terms: i64 = sql(c.query_row("SELECT COUNT(*) FROM terms", [], |r| r.get(0)))?;
    if v["extract_property_chains"].as_bool().unwrap_or(false) {
        rules.extend(property_chain_rules(&c)?);
        validate_rules(&mut rules)?;
        if rules.len() > 256 {
            return Err(err(
                "RESOURCE_LIMIT_EXCEEDED",
                "generated rule plan exceeds 256 rules",
            ));
        }
    }
    let resolved_import_lock = resolve_imports(&c, &v)?;
    let plan = Plan {
        schema_version: "ontology-native-plan/v1".into(),
        sccs: sccs(&rules),
        rules,
    };
    Ok(
        json!({"stats":{"parsed_facts":parsed,"explicit_facts":count,"indexed_facts":count,"distinct_terms":terms},"plan":plan,"resolved_import_lock":resolved_import_lock,"parser_options":parser_options,
        "metrics":{"parse_dictionary_seconds":parse_seconds,"index_seconds":start.elapsed().as_secs_f64()-parse_seconds,"engine_seconds":start.elapsed().as_secs_f64()}}),
    )
}

fn id(c: &Connection, s: &str) -> Result<i64> {
    if s == "DEFAULT" {
        return Ok(0);
    }
    sql(c.query_row("SELECT id FROM terms WHERE n=?", [s], |r| r.get(0)))
}
fn query(c: &Connection, r: &Rule, delta: usize, exclude_known: bool) -> Result<String> {
    let names = ["g", "s", "p", "o"];
    let mut bindings = BTreeMap::<String, String>::new();
    let mut conditions = Vec::new();
    let mut tables = Vec::new();
    for (i, a) in r.body.iter().enumerate() {
        tables.push(format!(
            "{} a{i}",
            if i == delta { "frontier" } else { "facts" }
        ));
        for (j, v) in a.iter().enumerate() {
            let col = format!("a{i}.{}", names[j]);
            if variable(v) {
                if let Some(first) = bindings.get(v) {
                    conditions.push(format!("{col}={first}"));
                } else {
                    bindings.insert(v.clone(), col);
                }
            } else {
                conditions.push(format!("{col}={}", id(c, v)?));
            }
        }
    }
    for f in &r.filters {
        let kind = match f.op.as_str() {
            "isIRI" => "kind='I'",
            "isResource" => "kind!='L'",
            _ => "kind='L'",
        };
        conditions.push(format!(
            "EXISTS(SELECT 1 FROM terms WHERE id={} AND {kind})",
            bindings[&f.var]
        ));
    }
    let h: Vec<String> = r
        .head
        .iter()
        .map(|v| {
            if variable(v) {
                Ok(bindings[v].clone())
            } else {
                id(c, v).map(|i| i.to_string())
            }
        })
        .collect::<Result<_>>()?;
    if exclude_known {
        conditions.push(format!("NOT EXISTS(SELECT 1 FROM facts established WHERE established.g={} AND established.s={} AND established.p={} AND established.o={})", h[0],h[1],h[2],h[3]));
    }
    Ok(format!(
        "SELECT {} FROM {}{}",
        h.join(","),
        tables.join(","),
        if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        }
    ))
}

fn materialize(v: Value) -> Result<Value> {
    let start = Instant::now();
    let database = v["database"]
        .as_str()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: database"))?;
    let memory_budget = v["memory_budget_bytes"].as_u64().ok_or_else(|| {
        err(
            "SOURCE_INVALID",
            "missing/invalid field: memory_budget_bytes",
        )
    })?;
    let disk_budget = v["disk_budget_bytes"]
        .as_u64()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: disk_budget_bytes"))?;
    let max_iter = v["max_iterations"]
        .as_u64()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: max_iterations"))?;
    let max_rows = v["max_intermediate_rows"].as_u64().ok_or_else(|| {
        err(
            "SOURCE_INVALID",
            "missing/invalid field: max_intermediate_rows",
        )
    })?;
    let max_derived = v["max_derived_facts"]
        .as_u64()
        .ok_or_else(|| err("SOURCE_INVALID", "missing/invalid field: max_derived_facts"))?;
    let mut plan: Plan =
        serde_json::from_value(v["plan"].clone()).map_err(|e| err("MANIFEST_CORRUPT", e))?;
    if plan.schema_version != "ontology-native-plan/v1" || plan.rules.len() > 256 {
        return Err(err(
            "MANIFEST_CORRUPT",
            "unsupported or oversized native plan",
        ));
    }
    validate_rules(&mut plan.rules)?;
    plan.sccs = sccs(&plan.rules);
    let mut c = setup(database, memory_budget, disk_budget)?;
    let timeout = v["timeout_seconds"].as_f64();
    sql(c.progress_handler(
        10_000,
        Some(move || timeout.is_some_and(|t| start.elapsed().as_secs_f64() > t)),
    ))?;
    sql(c.execute_batch("BEGIN; CREATE TABLE frontier(g INTEGER,s INTEGER,p INTEGER,o INTEGER,PRIMARY KEY(g,s,p,o)) WITHOUT ROWID;
        CREATE INDEX frontier_pos ON frontier(p,o,s,g); CREATE INDEX frontier_osp ON frontier(o,s,p,g);
        CREATE TABLE next(g INTEGER,s INTEGER,p INTEGER,o INTEGER,PRIMARY KEY(g,s,p,o)) WITHOUT ROWID;"))?;
    let explicit: i64 = sql(c.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)))?;
    let mut iterations = 0u64;
    let mut evaluations = 0u64;
    let mut candidates = 0u64;
    let mut inferred = 0u64;
    for group in plan.sccs {
        sql(c.execute_batch(
            "DELETE FROM frontier; INSERT INTO frontier SELECT g,s,p,o FROM facts;",
        ))?;
        loop {
            if iterations >= max_iter {
                return Err(err(
                    "FIXPOINT_LIMIT_EXCEEDED",
                    "max_iterations exhausted before fixpoint",
                ));
            }
            iterations += 1;
            sql(c.execute("DELETE FROM next", []))?;
            for &ri in &group {
                let r = &plan.rules[ri];
                for d in 0..r.body.len() {
                    let q = query(&c, r, d, false)?;
                    // Limit applies to raw join rows, before DISTINCT; counting a
                    // bounded subquery prevents an unbounded duplicate explosion.
                    let count: i64 = sql(c.query_row(
                        &format!("SELECT COUNT(*) FROM ({q} LIMIT {})", max_rows + 1),
                        [],
                        |r| r.get(0),
                    ))?;
                    if count as u64 > max_rows {
                        return Err(err(
                            "RESOURCE_LIMIT_EXCEEDED",
                            format!("{} exceeds intermediate rows", r.rule_id),
                        ));
                    }
                    candidates += count as u64;
                    evaluations += 1;
                    sql(c.execute_batch(&format!("INSERT OR IGNORE INTO next {q};")))?;
                    let next_count: i64 =
                        sql(c.query_row("SELECT COUNT(*) FROM next", [], |r| r.get(0)))?;
                    if next_count as u64 > max_rows {
                        return Err(err(
                            "RESOURCE_LIMIT_EXCEEDED",
                            "combined rule output exceeds intermediate-row budget",
                        ));
                    }
                }
            }
            let invalid:i64=sql(c.query_row("SELECT COUNT(*) FROM next n WHERE NOT EXISTS(SELECT 1 FROM terms WHERE id=n.s AND kind!='L') OR NOT EXISTS(SELECT 1 FROM terms WHERE id=n.p AND kind='I') OR (n.g!=0 AND NOT EXISTS(SELECT 1 FROM terms WHERE id=n.g AND kind!='L'))",[],|r|r.get(0)))?;
            if invalid > 0 {
                return Err(err(
                    "RULE_INVALID",
                    "rule derived a non-RDF subject, predicate or graph",
                ));
            }
            sql(c.execute_batch("DELETE FROM frontier; INSERT INTO frontier SELECT n.g,n.s,n.p,n.o FROM next n WHERE NOT EXISTS(SELECT 1 FROM facts f WHERE f.g=n.g AND f.s=n.s AND f.p=n.p AND f.o=n.o);"))?;
            let added: i64 = sql(c.query_row("SELECT COUNT(*) FROM frontier", [], |r| r.get(0)))?;
            if added == 0 {
                break;
            }
            inferred += added as u64;
            if inferred > max_derived {
                return Err(err("RESOURCE_LIMIT_EXCEEDED", "max_derived_facts exceeded"));
            }
            sql(c.execute_batch("INSERT INTO facts SELECT g,s,p,o,0 FROM frontier;"))?;
        }
    }
    sql(c.execute_batch("DROP TABLE next; DROP TABLE frontier; ANALYZE; COMMIT; VACUUM;"))?;
    Ok(
        json!({"stats":{"explicit_facts":explicit,"inferred_facts":inferred,"indexed_facts":explicit as u64+inferred,"fixpoint_iterations":iterations,"executed_rule_evaluations":evaluations},"fixpoint_reached":true,"metrics":{"rule_join_candidate_rows":candidates,"reasoning_seconds":start.elapsed().as_secs_f64()}}),
    )
}

pub fn execute(v: Value) -> Result<Value> {
    match v["operation"].as_str() {
        Some("compile") => compile(v),
        Some("materialize") => materialize(v),
        Some("rules") => admitted_rules(v),
        Some("identity") => Ok(runtime_info()),
        Some("query") => crate::disk::query(&v).map_err(|e| {
            let limited = e.to_string().contains("budget");
            err(
                if limited {
                    "BUDGET_EXHAUSTED"
                } else {
                    "QUERY_FAILED"
                },
                if limited {
                    format!("Disk query exceeded its cumulative read or decode budget: {e}")
                } else {
                    e.to_string()
                },
            )
        }),
        Some("validate") => validate_full(v),
        Some("prepare-query") => crate::query::local_query(
            v["query"]
                .as_str()
                .ok_or_else(|| err("QUERY_FAILED", "query"))?,
        )
        .map(|()| json!({"valid":true}))
        .map_err(|e| err("QUERY_FAILED", e)),
        _ => Err(err("NATIVE_ENGINE_FAILED", "unknown operation")),
    }
}

pub fn runtime_info() -> Value {
    json!({"distribution_version":env!("CARGO_PKG_VERSION"),"engine_semantic_version":"positive-rdf-v5",
        "format_version":"ontology-kernel-sqlite/v1","native_contract":"full-source-doc/v2",
        "execution_contract":crate::full_execution::CONTRACT,"artifact_contract":crate::full_artifact::MANIFEST_VERSION,
        "parser":"oxttl/0.2.5","term_model":"oxrdf/0.3.5","import_policy":"declared-ontology-v1","sqlite":rusqlite::version(),
        "kernel":crate::identity::kernel()})
}

fn admitted_rules(v: Value) -> Result<Value> {
    let profile = v["profile"]
        .as_str()
        .ok_or_else(|| err("RULE_INVALID", "profile required"))?;
    if ![
        "no-rules-v1",
        "datalog-v1",
        "rdfs-supported-v1",
        "rdfs-property-chain-v1",
    ]
    .contains(&profile)
    {
        return Err(err("UNSUPPORTED_RULE_PROFILE", "unsupported profile"));
    }
    let paths = v["files"]
        .as_array()
        .ok_or_else(|| err("RULE_INVALID", "rule files required"))?;
    if profile == "no-rules-v1" && !paths.is_empty() {
        return Err(err("RULE_INVALID", "no-rules-v1 does not accept rules"));
    }
    let mut rules = vec![];
    for path in paths.iter().filter_map(Value::as_str) {
        let path = std::path::Path::new(path);
        match path.extension().and_then(|s| s.to_str()) {
            Some("ttl") => {
                if profile != "rdfs-property-chain-v1" {
                    return Err(err(
                        "UNSUPPORTED_RULE_PROFILE",
                        "property chains require rdfs-property-chain-v1",
                    ));
                }
            }
            Some("rq") => {
                let text = std::fs::read_to_string(path).map_err(|e| err("RULE_INVALID", e))?;
                let query = spargebra::SparqlParser::new()
                    .parse_query(&text)
                    .map_err(|e| err("RULE_INVALID", e))?;
                if !matches!(
                    query,
                    spargebra::Query::Select { .. } | spargebra::Query::Ask { .. }
                ) {
                    return Err(err(
                        "UNSUPPORTED_RULE_PROFILE",
                        "only SELECT/ASK query assets are allowed",
                    ));
                }
            }
            _ => {
                let payload: Value = serde_json::from_slice(
                    &std::fs::read(path).map_err(|e| err("RULE_INVALID", e))?,
                )
                .map_err(|e| err("RULE_INVALID", e))?;
                if payload.as_object().is_none_or(|o| o.len() != 2)
                    || payload["schema_version"] != "ontology-rule-ir/v1"
                {
                    return Err(err("RULE_INVALID", "expected ontology-rule-ir/v1"));
                }
                rules.extend(
                    payload["rules"]
                        .as_array()
                        .ok_or_else(|| err("RULE_INVALID", "rules must be array"))?
                        .clone(),
                );
            }
        }
    }
    rules.extend(crate::full_rules::full_profile_rules(profile));
    let mut checked: Vec<Rule> =
        serde_json::from_value(json!(rules)).map_err(|e| err("RULE_INVALID", e))?;
    validate_rules(&mut checked)?;
    Ok(json!({"rules":rules,"metrics":{}}))
}

pub(crate) fn add_usage(reply: &mut Value) -> anyhow::Result<()> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // getrusage has no Python runtime or subprocess dependency.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
        let usage = unsafe { usage.assume_init() };
        {
            // Linux getrusage keeps the pre-exec high-water mark inherited from
            // the launcher. VmHWM belongs to the current executable's address
            // space, so a large gateway cannot exhaust a small Rust request.
            #[cfg(target_os = "linux")]
            let peak_rss = std::fs::read_to_string("/proc/self/status")?
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|value| value.split_whitespace().next())
                .ok_or_else(|| anyhow::anyhow!("missing Rust process VmHWM"))?
                .parse::<u64>()?
                * 1024;
            #[cfg(not(target_os = "linux"))]
            let peak_rss = usage.ru_maxrss as u64;
            reply["metrics"]["peak_rss_bytes"] = json!(peak_rss);
            reply["metrics"]["cpu_user_seconds"] =
                json!(usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1e6);
            reply["metrics"]["cpu_system_seconds"] =
                json!(usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1e6);
            reply["metrics"]["io_read_bytes"] = json!(usage.ru_inblock as u64 * 512);
            reply["metrics"]["io_write_bytes"] = json!(usage.ru_oublock as u64 * 512);
        }
    }
    Ok(())
}
fn validate_full(v: Value) -> Result<Value> {
    let data_path = std::path::Path::new(
        v["dataset"]
            .as_str()
            .ok_or_else(|| err("VALIDATION_FAILED", "dataset"))?,
    )
    .join("facts.sqlite");
    let c = Connection::open_with_flags(&data_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| err("VALIDATION_FAILED", e))?;
    let mut data = crate::rdf_graph::Graph {
        triples: BTreeSet::new(),
    };
    let mut statement=sql(c.prepare("SELECT DISTINCT s.n,p.n,o.n FROM facts f JOIN terms s ON s.id=f.s JOIN terms p ON p.id=f.p JOIN terms o ON o.id=f.o"))?;
    let rows = sql(statement.query_map([], |r| {
        Ok([
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ])
    }))?;
    for row in rows {
        data.triples.insert(sql(row)?);
    }
    let mut shapes = crate::rdf_graph::Graph {
        triples: BTreeSet::new(),
    };
    if v["shapes_from_dataset"].as_bool() == Some(true) {
        if v["shapes"]
            .as_array()
            .is_some_and(|sources| !sources.is_empty())
        {
            return Err(err(
                "SOURCE_INVALID",
                "choose dataset shapes or external shape documents",
            ));
        }
        shapes = data.clone();
    }
    for source in v["shapes"]
        .as_array()
        .ok_or_else(|| err("VALIDATION_FAILED", "shapes"))?
        .iter()
    {
        use oxigraph::io::{RdfFormat, RdfParser};
        let format = match source["format"].as_str() {
            Some("turtle") => RdfFormat::Turtle,
            Some("ntriples") => RdfFormat::NTriples,
            Some("trig") => RdfFormat::TriG,
            Some("nquads") => RdfFormat::NQuads,
            _ => return Err(err("SOURCE_INVALID", "shape format")),
        };
        let mut parser = RdfParser::from_format(format).rename_blank_nodes();
        if let Some(base) = source["base_iri"].as_str() {
            parser = parser
                .with_base_iri(base)
                .map_err(|e| err("SOURCE_INVALID", e))?;
        }
        let bytes = std::fs::read(
            source["file"]
                .as_str()
                .ok_or_else(|| err("SOURCE_INVALID", "shape file"))?,
        )
        .map_err(|e| err("SOURCE_INVALID", e))?;
        for quad in parser.for_reader(bytes.as_slice()) {
            let q = quad.map_err(|e| err("SOURCE_INVALID", e))?;
            shapes.triples.insert([
                q.subject.to_string(),
                q.predicate.to_string(),
                q.object.to_string(),
            ]);
        }
    }
    let selected = shapes
        .triples
        .iter()
        .filter(|t| {
            t[1] == crate::rdf_graph::iri(&format!("{}type", crate::rdf_graph::RDF))
                && ["NodeShape", "PropertyShape"].iter().any(|kind| {
                    t[2] == crate::rdf_graph::iri(&format!("{}{kind}", crate::rdf_graph::SH))
                })
        })
        .map(|t| t[0].clone())
        .collect::<BTreeSet<_>>();
    let selected = match v.get("selected_shapes") {
        None => selected,
        Some(value) => rdf_resources(value, "selected_shapes")?,
    };
    let focus = match v.get("focus_node") {
        None | Some(Value::Null) => None,
        Some(value) => Some(rdf_resource(value, "focus_node")?),
    };
    let max_depth = match v.get("max_validation_depth") {
        None => 64,
        Some(value) => value
            .as_u64()
            .filter(|depth| *depth > 0)
            .ok_or_else(|| err("SOURCE_INVALID", "positive max_validation_depth required"))?
            as usize,
    };
    let result = crate::shacl::validate_with_depth(
        &data,
        &shapes,
        &selected,
        focus.as_deref(),
        max_depth,
        true,
    )
    .map_err(|e| {
        err(
            if e.to_string().contains("unsupported") || e.to_string().contains("external query") {
                "UNSUPPORTED_RULE_PROFILE"
            } else {
                "VALIDATION_FAILED"
            },
            e,
        )
    })?;
    let checked = selected.len();
    Ok(
        json!({"conforms":result["conforms"],"checked_shapes":checked,"violations":result["violations"].as_array().map_or(0,Vec::len),"report":serde_json::to_string(&result).map_err(|e|err("VALIDATION_FAILED",e))?,"metrics":{},"kernel":crate::identity::kernel()}),
    )
}

fn rdf_resource(value: &Value, field: &str) -> Result<String> {
    let text = value.as_str().ok_or_else(|| err("SOURCE_INVALID", field))?;
    let term = text
        .parse::<Term>()
        .map_err(|error| err("SOURCE_INVALID", error))?;
    match term {
        Term::NamedNode(_) | Term::BlankNode(_) => Ok(term.to_string()),
        _ => Err(err(
            "SOURCE_INVALID",
            format!("{field} must be an RDF resource"),
        )),
    }
}

fn rdf_resources(value: &Value, field: &str) -> Result<BTreeSet<String>> {
    value
        .as_array()
        .ok_or_else(|| err("SOURCE_INVALID", field))?
        .iter()
        .map(|item| rdf_resource(item, field))
        .collect()
}
