//! Offline, positive binary rules: head(x,y) <- p(x,y) or head(x,z) <- p(x,y), q(y,z).
use anyhow::{Result, ensure};
use datafrog::{Iteration, Variable};
use ontokiln_pack::{Id, Pack};
use oxrdf::NamedNode;
use petgraph::{
    algo::{condensation, toposort},
    graph::DiGraph,
};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    io::Write,
};

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub head: String,
    pub body: Vec<String>,
}

#[derive(Clone, Copy)]
pub enum Engine {
    Datafrog,
    #[cfg(feature = "ascent-engine")]
    Ascent,
}

#[derive(Clone, Copy)]
struct Op {
    head: usize,
    left: usize,
    right: Option<usize>,
}

type Pair = (Id, Id);
type Relations = Vec<BTreeSet<Pair>>;

pub struct Summary {
    pub stages: Vec<Vec<String>>,
    pub added: usize,
}

pub fn materialize(
    pack: &Pack,
    rules: &[Rule],
    engine: Engine,
    max_terms: usize,
    mut output: impl Write,
) -> Result<Summary> {
    ensure!(max_terms > 0, "max_terms must be positive");
    ensure!((1..=32).contains(&rules.len()), "expected 1..32 rules");
    let mut names = BTreeSet::new();
    for rule in rules {
        ensure!(
            (1..=2).contains(&rule.body.len()),
            "each body must have one or two predicates"
        );
        for name in std::iter::once(&rule.head).chain(&rule.body) {
            NamedNode::new(name.as_str())?;
            names.insert(name.as_str());
        }
    }
    let names: Vec<_> = names.into_iter().collect();
    let mut graph = DiGraph::new();
    let nodes: Vec<_> = (0..names.len()).map(|id| graph.add_node(id)).collect();
    let mut ops = Vec::new();
    for r in rules {
        let id = |s: &String| names.binary_search(&s.as_str()).unwrap();
        let head = id(&r.head);
        for body in &r.body {
            graph.update_edge(nodes[id(body)], nodes[head], ());
        }
        ops.push(Op {
            head,
            left: id(&r.body[0]),
            right: r.body.get(1).map(id),
        });
    }
    // Each SCC is a mutually recursive group. Predecessors reach their fixed point first.
    let dag = condensation(graph, true);
    let order = toposort(&dag, None).expect("SCC condensation is acyclic");
    let predicates: HashMap<Id, usize> = names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| pack.lookup(&format!("<{name}>")).map(|p| (p, i)))
        .collect();
    let mut initial = vec![BTreeSet::new(); names.len()];
    let mut domain = BTreeSet::new();
    for &[s, p, o] in pack.triples() {
        if let Some(&rel) = predicates.get(&p) {
            domain.insert(s);
            domain.insert(o);
            // A bound before evaluating either engine also limits dense intermediate joins.
            ensure!(
                domain.len() <= max_terms,
                "offline rule domain exceeds {max_terms} distinct terms"
            );
            initial[rel].insert((s, o));
        }
    }
    let mut facts = initial.clone();
    let mut stages = Vec::new();
    for component in order {
        let group = &dag[component];
        let rules: Vec<_> = ops
            .iter()
            .copied()
            .filter(|op| group.contains(&op.head))
            .collect();
        if rules.is_empty() {
            continue;
        }
        let solved = match engine {
            Engine::Datafrog => datafrog_stage(&facts, &rules),
            #[cfg(feature = "ascent-engine")]
            Engine::Ascent => ascent_stage(&facts, &rules),
        };
        for &id in group {
            facts[id].clone_from(&solved[id]);
        }
        stages.push(group.iter().map(|&i| names[i].to_owned()).collect());
    }
    let mut added = 0;
    for (i, rows) in facts.iter().enumerate() {
        for &(s, o) in rows.difference(&initial[i]) {
            writeln!(output, "{} <{}> {} .", pack.term(s), names[i], pack.term(o))?;
            added += 1;
        }
    }
    Ok(Summary { stages, added })
}

fn datafrog_stage(seed: &Relations, rules: &[Op]) -> Relations {
    let mut iteration = Iteration::new();
    let forward: Vec<Variable<Pair>> = seed
        .iter()
        .map(|rows| {
            let rel = iteration.variable("forward");
            rel.extend(rows.iter().copied());
            rel
        })
        .collect();
    let reverse: Vec<Variable<Pair>> = (0..seed.len())
        .map(|_| iteration.variable("by_object"))
        .collect();
    let left_keys: BTreeSet<_> = rules
        .iter()
        .filter(|r| r.right.is_some())
        .map(|r| r.left)
        .collect();
    while iteration.changed() {
        for &i in &left_keys {
            reverse[i].from_map(&forward[i], |&(s, o)| (o, s));
        }
        for rule in rules {
            let dest = &forward[rule.head];
            if let Some(right) = rule.right {
                dest.from_join(&reverse[rule.left], &forward[right], |_, &s, &o| (s, o));
            } else {
                dest.from_map(&forward[rule.left], |&pair| pair);
            }
        }
    }
    forward
        .into_iter()
        .map(|r| r.complete().elements.into_iter().collect())
        .collect()
}

#[cfg(feature = "ascent-engine")]
fn ascent_stage(seed: &Relations, rules: &[Op]) -> Relations {
    ascent::ascent! {
        struct Stage;
        relation fact(usize,Id,Id);
        relation copy(usize,usize);
        relation chain(usize,usize,usize);
        fact(h,x,y) <-- copy(h,p), fact(p,x,y);
        fact(h,x,z) <-- chain(h,p,q), fact(p,x,y), fact(q,y,z);
    }
    let mut stage = Stage::default();
    for (i, rows) in seed.iter().enumerate() {
        stage.fact.extend(rows.iter().map(|&(s, o)| (i, s, o)));
    }
    for rule in rules {
        if let Some(right) = rule.right {
            stage.chain.push((rule.head, rule.left, right));
        } else {
            stage.copy.push((rule.head, rule.left));
        }
    }
    stage.run();
    let mut facts = vec![BTreeSet::new(); seed.len()];
    for (p, s, o) in stage.fact {
        facts[p].insert((s, o));
    }
    facts
}
