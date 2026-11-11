//! Parameters are RDF terms placed at explicit IRI slots in the parsed algebra.
//! Variables retain SPARQL's lexical scope, including inside SELECT and EXISTS.
use anyhow::{Result, bail, ensure};
use oxrdf::{Literal, NamedNode};
use serde::{Deserialize, Serialize};
use spargebra::{
    Query,
    algebra::{
        AggregateExpression, Expression as E, GraphPattern as G, OrderExpression,
        PropertyPathExpression as P,
    },
    term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Parameter {
    #[serde(alias = "uri")]
    Iri { value: String },
    Literal {
        value: String,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        datatype: Option<String>,
    },
}
impl Parameter {
    fn term(&self) -> Result<GroundTerm> {
        Ok(match self {
            Self::Iri { value } => NamedNode::new(value.clone())?.into(),
            Self::Literal {
                value,
                language,
                datatype,
            } => match (language, datatype) {
                (None, None) => Literal::new_simple_literal(value.clone()),
                (Some(lang), None) => {
                    Literal::new_language_tagged_literal(value.clone(), lang.clone())?
                }
                (None, Some(ty)) => {
                    Literal::new_typed_literal(value.clone(), NamedNode::new(ty.clone())?)
                }
                _ => bail!("literal accepts either language or datatype"),
            }
            .into(),
        })
    }
}

struct Slots<'a> {
    params: &'a BTreeMap<String, Parameter>,
    used: BTreeSet<String>,
}

pub fn bind(mut query: Query, params: &BTreeMap<String, Parameter>) -> Result<Query> {
    let mut slots = Slots {
        params,
        used: BTreeSet::new(),
    };
    if let Some(dataset) = query.dataset_mut() {
        for graph in dataset
            .default
            .iter_mut()
            .chain(dataset.named.iter_mut().flatten())
        {
            slots.iri(graph)?;
        }
    }
    let pattern = match &mut query {
        Query::Construct {
            template, pattern, ..
        } => {
            for t in template {
                slots.triple(t)?;
            }
            pattern
        }
        Query::Ask { pattern, .. }
        | Query::Select { pattern, .. }
        | Query::Describe { pattern, .. } => pattern,
    };
    slots.pattern(pattern)?;
    ensure!(
        slots.used.len() == params.len(),
        "undeclared template parameter"
    );
    Ok(query)
}
impl Slots<'_> {
    fn lookup(&mut self, node: &NamedNode) -> Result<Option<GroundTerm>> {
        let Some(name) = node.as_str().strip_prefix("urn:ontokiln:param:") else {
            return Ok(None);
        };
        let Some(value) = self.params.get(name) else {
            bail!("missing template parameter {name}")
        };
        self.used.insert(name.into());
        Ok(Some(value.term()?))
    }
    fn iri(&mut self, node: &mut NamedNode) -> Result<()> {
        if let Some(value) = self.lookup(node)? {
            let GroundTerm::NamedNode(iri) = value else {
                bail!("IRI parameter required at {node}")
            };
            *node = iri;
        }
        Ok(())
    }
    fn term(&mut self, t: &mut TermPattern, subject: bool) -> Result<()> {
        if let TermPattern::NamedNode(n) = t
            && let Some(value) = self.lookup(n)?
        {
            *t = match value {
                GroundTerm::NamedNode(n) => n.into(),
                GroundTerm::Literal(l) => {
                    ensure!(!subject, "subject requires an IRI");
                    l.into()
                }
                #[allow(unreachable_patterns)]
                _ => bail!("parameters require an IRI or literal"),
            };
        }
        Ok(())
    }
    fn triple(&mut self, t: &mut TriplePattern) -> Result<()> {
        self.term(&mut t.subject, true)?;
        if let NamedNodePattern::NamedNode(n) = &mut t.predicate {
            self.iri(n)?;
        }
        self.term(&mut t.object, false)
    }
    fn path(&mut self, p: &mut P) -> Result<()> {
        match p {
            P::NamedNode(n) => self.iri(n)?,
            P::Reverse(p) | P::ZeroOrMore(p) | P::OneOrMore(p) | P::ZeroOrOne(p) => self.path(p)?,
            P::Sequence(l, r) | P::Alternative(l, r) => {
                self.path(l)?;
                self.path(r)?
            }
            P::NegatedPropertySet(v) => {
                for n in v {
                    self.iri(n)?
                }
            }
        }
        Ok(())
    }
    fn expr(&mut self, e: &mut E) -> Result<()> {
        match e {
            E::NamedNode(n) => {
                if let Some(v) = self.lookup(n)? {
                    *e = match v {
                        GroundTerm::NamedNode(n) => E::NamedNode(n),
                        GroundTerm::Literal(l) => E::Literal(l),
                        #[allow(unreachable_patterns)]
                        _ => bail!("parameters require an IRI or literal"),
                    }
                }
            }
            E::Exists(g) => self.pattern(g)?,
            E::Or(l, r)
            | E::And(l, r)
            | E::Equal(l, r)
            | E::SameTerm(l, r)
            | E::Greater(l, r)
            | E::GreaterOrEqual(l, r)
            | E::Less(l, r)
            | E::LessOrEqual(l, r)
            | E::Add(l, r)
            | E::Subtract(l, r)
            | E::Multiply(l, r)
            | E::Divide(l, r) => {
                self.expr(l)?;
                self.expr(r)?
            }
            E::In(l, items) => {
                self.expr(l)?;
                for x in items {
                    self.expr(x)?
                }
            }
            E::UnaryPlus(x) | E::UnaryMinus(x) | E::Not(x) => self.expr(x)?,
            E::If(c, t, f) => {
                self.expr(c)?;
                self.expr(t)?;
                self.expr(f)?
            }
            E::Coalesce(v) | E::FunctionCall(_, v) => {
                for x in v {
                    self.expr(x)?
                }
            }
            E::Variable(_) | E::Literal(_) | E::Bound(_) => {}
        }
        Ok(())
    }
    fn pattern(&mut self, g: &mut G) -> Result<()> {
        match g {
            G::Bgp { patterns } => {
                for t in patterns {
                    self.triple(t)?
                }
            }
            G::Path {
                subject,
                path,
                object,
            } => {
                self.term(subject, true)?;
                self.path(path)?;
                self.term(object, false)?
            }
            G::Join { left, right } | G::Union { left, right } | G::Minus { left, right } => {
                self.pattern(left)?;
                self.pattern(right)?
            }
            G::LeftJoin {
                left,
                right,
                expression,
            } => {
                self.pattern(left)?;
                self.pattern(right)?;
                if let Some(x) = expression {
                    self.expr(x)?
                }
            }
            G::Filter { expr, inner }
            | G::Extend {
                expression: expr,
                inner,
                ..
            } => {
                self.expr(expr)?;
                self.pattern(inner)?
            }
            G::Project { inner, .. }
            | G::Distinct { inner }
            | G::Reduced { inner }
            | G::Slice { inner, .. } => self.pattern(inner)?,
            G::OrderBy { inner, expression } => {
                for OrderExpression::Asc(e) | OrderExpression::Desc(e) in expression {
                    self.expr(e)?
                }
                self.pattern(inner)?
            }
            G::Group {
                inner, aggregates, ..
            } => {
                for (_, a) in aggregates {
                    if let AggregateExpression::FunctionCall { expr, .. } = a {
                        self.expr(expr)?
                    }
                }
                self.pattern(inner)?
            }
            G::Graph { name, inner } | G::Service { name, inner, .. } => {
                if let NamedNodePattern::NamedNode(n) = name {
                    self.iri(n)?
                }
                self.pattern(inner)?
            }
            G::Values { bindings, .. } => {
                for row in bindings {
                    for cell in row.iter_mut().flatten() {
                        if let GroundTerm::NamedNode(n) = cell
                            && let Some(v) = self.lookup(n)?
                        {
                            *cell = v
                        }
                    }
                }
            }
            #[allow(unreachable_patterns)]
            _ => bail!("unsupported SPARQL graph pattern"),
        }
        Ok(())
    }
}
