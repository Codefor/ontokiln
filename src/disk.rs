//! Read immutable Full artifacts with the same SPARQL evaluator as packs.
use anyhow::{Context, Result, ensure};
use oxigraph::model::{Term, Variable};
use oxigraph::sparql::SparqlEvaluator;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};
use serde_json::{Value, json};
use spareval::{InternalQuad, QueryableDataset};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Encoded {
    Stored(i64),
    External(Term),
}
struct Disk {
    db: Connection,
    inferred: bool,
    asserted: bool,
    deadline: Option<Instant>,
    rows: Cell<usize>,
    bytes: Cell<usize>,
    cache: RefCell<HashMap<i64, Term>>,
    encoded_cache: RefCell<HashMap<Term, Encoded>>,
    overlay: Vec<[Encoded; 3]>,
    overlay_set: HashSet<[Encoded; 3]>,
    overlay_index: [HashMap<Encoded, Vec<usize>>; 3],
}
impl Disk {
    fn charge(&self, rows: usize, bytes: usize) -> io::Result<()> {
        self.rows.set(self.rows.get() + rows);
        self.bytes.set(self.bytes.get() + bytes);
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() > deadline)
        {
            return Err(io::Error::other(
                "BUDGET_EXHAUSTED: Disk query exceeded its deadline",
            ));
        }
        Ok(())
    }
    fn decode(&self, id: i64) -> io::Result<Term> {
        self.charge(0, 0)?;
        if let Some(t) = self.cache.borrow().get(&id) {
            return Ok(t.clone());
        }
        let text: String = self
            .db
            .query_row("SELECT n FROM terms WHERE id=?", [id], |r| r.get(0))
            .map_err(io::Error::other)?;
        self.charge(0, text.len() + 128)?;
        let term = text.parse::<Term>().map_err(io::Error::other)?;
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(id, term.clone());
        Ok(term)
    }
}
impl<'a> QueryableDataset<'a> for &'a Disk {
    type InternalTerm = Encoded;
    type Error = io::Error;
    fn internal_quads_for_pattern(
        &self,
        s: Option<&Encoded>,
        p: Option<&Encoded>,
        o: Option<&Encoded>,
        g: Option<Option<&Encoded>>,
    ) -> impl Iterator<Item = io::Result<InternalQuad<Encoded>>> + use<'a> {
        let disk = *self;
        let mut overlay = Vec::new();
        if g == Some(None) {
            let candidates = [(0, s), (1, p), (2, o)]
                .into_iter()
                .filter_map(|(column, term)| {
                    term.map(|term| {
                        disk.overlay_index[column]
                            .get(term)
                            .map(Vec::as_slice)
                            .unwrap_or(&[])
                    })
                })
                .min_by_key(|rows| rows.len());
            let all;
            let candidates = if let Some(rows) = candidates {
                rows
            } else {
                all = (0..disk.overlay.len()).collect::<Vec<_>>();
                &all
            };
            for &index in candidates {
                let t = &disk.overlay[index];
                if s.is_none_or(|s| s == &t[0])
                    && p.is_none_or(|p| p == &t[1])
                    && o.is_none_or(|o| o == &t[2])
                {
                    overlay.push(Ok(InternalQuad {
                        subject: t[0].clone(),
                        predicate: t[1].clone(),
                        object: t[2].clone(),
                        graph_name: None,
                    }))
                }
            }
        }
        let mut clauses = vec![];
        let mut args = vec![];
        let mut impossible = false;
        for (col, term) in [("s", s), ("p", p), ("o", o)] {
            match term {
                Some(Encoded::Stored(id)) => {
                    clauses.push(format!("{col}=?"));
                    args.push(*id);
                }
                Some(Encoded::External(_)) => impossible = true,
                None => {}
            }
        }
        if disk.inferred {
            clauses.push("explicit=0".into());
        }
        if disk.asserted {
            clauses.push("explicit=1".into());
        }
        match g {
            Some(None) => {}
            Some(Some(Encoded::Stored(id))) => {
                clauses.push("g=?".into());
                args.push(*id);
            }
            Some(Some(Encoded::External(_))) => impossible = true,
            None => clauses.push("g>0".into()),
        }
        let union = g == Some(None);
        let columns = if union { "s,p,o,0" } else { "s,p,o,g" };
        let mut after = [0_i64; 4];
        let mut page = Vec::new().into_iter();
        let mut finished = impossible;
        std::iter::from_fn(move || {
            if let Some(row) = page.next() {
                return Some(row);
            }
            if finished {
                return None;
            }
            if let Err(e) = disk.charge(0, 0) {
                finished = true;
                return Some(Err(e));
            }
            let mut filter = clauses.clone();
            filter.push(
                if union {
                    "(s,p,o)>(?,?,?)"
                } else {
                    "(s,p,o,g)>(?,?,?,?)"
                }
                .into(),
            );
            let mut bindings = args.clone();
            bindings.extend_from_slice(&after[..if union { 3 } else { 4 }]);
            let page_size = 256;
            let sql = format!(
                "SELECT DISTINCT {columns} FROM facts WHERE {} ORDER BY {} LIMIT {page_size}",
                filter.join(" AND "),
                if union { "s,p,o" } else { "s,p,o,g" }
            );
            let rows = (|| -> io::Result<Vec<[i64; 4]>> {
                let mut statement = disk.db.prepare(&sql).map_err(io::Error::other)?;
                statement
                    .query_map(params_from_iter(bindings), |r| {
                        Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])
                    })
                    .map_err(io::Error::other)?
                    .map(|r| r.map_err(io::Error::other))
                    .collect()
            })();
            match rows {
                Err(e) => {
                    finished = true;
                    Some(Err(e))
                }
                Ok(rows) => {
                    finished = rows.len() < page_size;
                    if let Some(last) = rows.last() {
                        after = *last;
                    } else {
                        return None;
                    }
                    page = rows
                        .into_iter()
                        .map(|q| {
                            disk.charge(1, 192)?;
                            Ok(InternalQuad {
                                subject: Encoded::Stored(q[0]),
                                predicate: Encoded::Stored(q[1]),
                                object: Encoded::Stored(q[2]),
                                graph_name: if union {
                                    None
                                } else {
                                    Some(Encoded::Stored(q[3]))
                                },
                            })
                        })
                        .collect::<Vec<_>>()
                        .into_iter();
                    page.next()
                }
            }
        })
        .filter(move |row| {
            if let Ok(q) = row
                && union
            {
                return !disk.overlay_set.contains(&[
                    q.subject.clone(),
                    q.predicate.clone(),
                    q.object.clone(),
                ]);
            }
            true
        })
        .chain(overlay)
    }
    fn internal_named_graphs(&self) -> impl Iterator<Item = io::Result<Encoded>> + use<'a> {
        let rows = (|| -> io::Result<Vec<Encoded>> {
            self.charge(0, 0)?;
            let mut statement = self
                .db
                .prepare("SELECT DISTINCT g FROM facts WHERE g>0 ORDER BY g")
                .map_err(io::Error::other)?;
            statement
                .query_map([], |r| r.get::<_, i64>(0))
                .map_err(io::Error::other)?
                .map(|r| {
                    self.charge(1, 32)?;
                    Ok(Encoded::Stored(r.map_err(io::Error::other)?))
                })
                .collect()
        })();
        match rows {
            Ok(rows) => rows.into_iter().map(Ok).collect::<Vec<_>>(),
            Err(e) => vec![Err(e)],
        }
        .into_iter()
    }
    fn contains_internal_graph_name(&self, g: &Encoded) -> io::Result<bool> {
        self.charge(0, 0)?;
        match g {
            Encoded::Stored(id) => Ok(self
                .db
                .query_row("SELECT 1 FROM facts WHERE g=? LIMIT 1", [id], |_| Ok(()))
                .optional()
                .map_err(io::Error::other)?
                .is_some()),
            _ => Ok(false),
        }
    }
    fn internalize_term(&self, t: Term) -> io::Result<Encoded> {
        self.charge(0, 0)?;
        if let Some(value) = self.encoded_cache.borrow().get(&t) {
            return Ok(value.clone());
        }
        let encoded = self
            .db
            .query_row("SELECT id FROM terms WHERE n=?", [t.to_string()], |r| {
                r.get(0)
            })
            .optional()
            .map_err(io::Error::other)?
            .map_or_else(|| Encoded::External(t.clone()), Encoded::Stored);
        let mut cache = self.encoded_cache.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(t, encoded.clone());
        Ok(encoded)
    }
    fn externalize_term(&self, t: Encoded) -> io::Result<Term> {
        match t {
            Encoded::Stored(id) => self.decode(id),
            Encoded::External(t) => Ok(t),
        }
    }
}

pub fn query(a: &Value) -> Result<Value> {
    let path = Path::new(a["dataset"].as_str().context("dataset path")?);
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let seconds = if a.get("query_seconds").is_some_and(Value::is_null) {
        None
    } else {
        Some(a["query_seconds"].as_f64().unwrap_or(30.0))
    };
    ensure!(
        seconds.is_none_or(|seconds| seconds.is_finite() && seconds > 0.0),
        "invalid query deadline"
    );
    let deadline = seconds.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds));
    db.progress_handler(
        1000,
        Some(move || deadline.is_some_and(|deadline| Instant::now() > deadline)),
    )?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;")?;
    let view = a["view"].as_str().unwrap_or("closure");
    ensure!(
        ["asserted", "closure", "inferred"].contains(&view),
        "unknown graph view"
    );
    let mut disk = Disk {
        db,
        inferred: view == "inferred",
        asserted: view == "asserted",
        deadline,
        rows: Cell::new(0),
        bytes: Cell::new(0),
        cache: RefCell::new(HashMap::new()),
        encoded_cache: RefCell::new(HashMap::new()),
        overlay: Vec::new(),
        overlay_set: HashSet::new(),
        overlay_index: Default::default(),
    };
    let triples: Vec<[Term; 3]> = a["overlay"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let cells = row.as_array().context("overlay triple")?;
                    ensure!(cells.len() == 3, "overlay triple");
                    Ok([
                        cells[0].as_str().context("overlay term")?.parse()?,
                        cells[1].as_str().context("overlay term")?.parse()?,
                        cells[2].as_str().context("overlay term")?.parse()?,
                    ])
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    for triple in triples {
        let terms = [
            (&disk).internalize_term(triple[0].clone())?,
            (&disk).internalize_term(triple[1].clone())?,
            (&disk).internalize_term(triple[2].clone())?,
        ];
        if disk.overlay_set.insert(terms.clone()) {
            for (column, term) in terms.iter().enumerate() {
                disk.overlay_index[column]
                    .entry(term.clone())
                    .or_default()
                    .push(disk.overlay.len());
            }
            disk.overlay.push(terms);
        }
    }
    let text = a["query"].as_str().context("query")?;
    ensure!(
        a.get("params")
            .is_none_or(|v| v.is_null() || v.as_object().is_some_and(|v| v.is_empty())),
        "use typed parameters with explicit urn:ontokiln:param: slots"
    );
    if a["derived_data"] == true {
        crate::query::local_derived_query(text)?;
    } else {
        crate::query::local_query(text)?;
    }
    let params = serde_json::from_value(a.get("parameters").cloned().unwrap_or(json!({})))?;
    let tables: Vec<ontokiln_reasoner::parameters::ValuesTable> =
        serde_json::from_value(a.get("values").cloned().unwrap_or(json!([])))?;
    let mut parsed = ontokiln_reasoner::parameters::bind_tables(
        spargebra::SparqlParser::new().parse_query(text)?,
        &params,
        &tables,
    )?;
    let bindings = a["rdf_bindings"].as_object();
    let columns = if let Some(bindings) = bindings.filter(|b| !b.is_empty()) {
        crate::query::expose_bindings(&mut parsed, bindings.keys().cloned())?
    } else {
        None
    };
    // Do not fold FILTER/BOUND input variables to unbound before applying their values.
    let evaluator = if bindings.is_some_and(|b| !b.is_empty()) {
        SparqlEvaluator::new().without_optimizations()
    } else {
        SparqlEvaluator::new()
    };
    let mut prepared = evaluator.for_query(parsed);
    if let Some(bindings) = a["rdf_bindings"].as_object() {
        for (name, t) in bindings {
            prepared = prepared.substitute_variable(
                Variable::new(name)?,
                t.as_str().context("RDF binding")?.parse::<Term>()?,
            );
        }
    }
    let mut result = crate::query::collect(
        prepared.on_queryable_dataset(&disk).execute()?,
        a["max_rows"].as_u64().unwrap_or(100_000) as usize,
        a["max_bytes"].as_u64().unwrap_or(64 * 1024 * 1024) as usize,
        view,
    )?;
    if let Some(columns) = columns {
        for row in result["items"].as_array_mut().context("query rows")? {
            row.as_object_mut()
                .context("query row")?
                .retain(|name, _| columns.contains(name));
        }
        result["variables"] = json!(columns);
    }
    disk.charge(
        result["items"].as_array().map_or(0, Vec::len),
        serde_json::to_vec(&result)?.len(),
    )?;
    result["read_metrics"] = json!({"rows":disk.rows.get(),"bytes":disk.bytes.get()});
    Ok(result)
}
