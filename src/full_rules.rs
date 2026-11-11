use serde_json::{Value, json};
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Rule {
    pub name: String,
    pub head: [String; 3],
    pub body: Vec<[String; 3]>,
}

fn rdfs_rules() -> Vec<Rule> {
    let r = "http://www.w3.org/2000/01/rdf-schema#";
    let t = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let sc = format!("<{r}subClassOf>");
    let sp = format!("<{r}subPropertyOf>");
    let domain = format!("<{r}domain>");
    let range = format!("<{r}range>");
    let triple = |s: &str, p: &str, o: &str| [s.into(), p.into(), o.into()];
    vec![
        Rule {
            name: "rdfs-subclass-transitive".into(),
            head: triple("?a", &sc, "?c"),
            body: vec![triple("?a", &sc, "?b"), triple("?b", &sc, "?c")],
        },
        Rule {
            name: "rdfs-type-inheritance".into(),
            head: triple("?x", t, "?b"),
            body: vec![triple("?x", t, "?a"), triple("?a", &sc, "?b")],
        },
        Rule {
            name: "rdfs-subproperty-transitive".into(),
            head: triple("?a", &sp, "?c"),
            body: vec![triple("?a", &sp, "?b"), triple("?b", &sp, "?c")],
        },
        Rule {
            name: "rdfs-property-inheritance".into(),
            head: triple("?x", "?q", "?y"),
            body: vec![triple("?p", &sp, "?q"), triple("?x", "?p", "?y")],
        },
        Rule {
            name: "rdfs-domain".into(),
            head: triple("?x", t, "?c"),
            body: vec![triple("?p", &domain, "?c"), triple("?x", "?p", "?y")],
        },
        Rule {
            name: "rdfs-range".into(),
            head: triple("?y", t, "?c"),
            body: vec![triple("?p", &range, "?c"), triple("?x", "?p", "?y")],
        },
    ]
}

pub fn full_profile_rules(profile: &str) -> Vec<Value> {
    if !["rdfs-supported-v1", "rdfs-property-chain-v1"].contains(&profile) {
        return vec![];
    }
    let ids = [
        "subclass-transitive",
        "type",
        "subproperty-transitive",
        "property",
        "domain",
        "range",
    ];
    let mut rules=rdfs_rules().into_iter().zip(ids).map(|(r,id)|json!({"rule_id":format!("rdfs-v1:{id}"),"head":r.head,"body":r.body,"filters":if id=="range"{vec![json!({"op":"isResource","var":"?y"})]}else{vec![]}})).collect::<Vec<_>>();
    if profile == "rdfs-property-chain-v1" {
        let inverse = "<http://www.w3.org/2002/07/owl#inverseOf>";
        let rdf = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        for (id, head, body, filters) in [
            (
                "inverse-forward",
                json!(["?y", "?q", "?x"]),
                json!([["?p", inverse, "?q"], ["?x", "?p", "?y"]]),
                json!([{"op":"isIRI","var":"?p"},{"op":"isIRI","var":"?q"},{"op":"isResource","var":"?y"}]),
            ),
            (
                "inverse-reverse",
                json!(["?y", "?p", "?x"]),
                json!([["?p", inverse, "?q"], ["?x", "?q", "?y"]]),
                json!([{"op":"isIRI","var":"?p"},{"op":"isIRI","var":"?q"},{"op":"isResource","var":"?y"}]),
            ),
            (
                "symmetric",
                json!(["?y", "?p", "?x"]),
                json!([
                    [
                        "?p",
                        rdf,
                        "<http://www.w3.org/2002/07/owl#SymmetricProperty>"
                    ],
                    ["?x", "?p", "?y"]
                ]),
                json!([{"op":"isResource","var":"?y"}]),
            ),
        ] {
            rules.push(json!({"rule_id":format!("rdfs-v1:{id}"),"head":head,"body":body,"filters":filters}));
        }
    }
    rules
}
