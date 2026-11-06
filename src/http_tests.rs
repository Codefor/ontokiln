use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request as HttpRequest,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn post(app: Router, path: &str, payload: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            HttpRequest::post(path)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn compiled_templates_filters_and_ordered_batch_use_the_same_snapshot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let app = router(App {
        engine: Arc::new(Reasoner::new(
            compile(&root.join("examples/scenarios/ontology")).unwrap(),
        )),
        slots: Arc::new(Semaphore::new(2)),
        source: "ttl",
        templates: Arc::new(queries::templates(Some(&root.join("examples/queries"))).unwrap()),
    });
    let (status, json) = post(
        app.clone(),
        "/v1/sparql",
        json!({"template":"inputs","bindings":{"step":"<urn:ontokiln:lab:FormatAddress>"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = json["results"]["bindings"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["field"]["value"], "urn:ontokiln:lab:Order.city");
    assert_eq!(rows[1]["field"]["value"], "urn:ontokiln:lab:Order.detail");
    assert_eq!(
        post(app.clone(), "/v1/sparql", json!({"template":"../inputs"}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            app.clone(),
            "/v1/sparql",
            json!({"template":"inputs","bindings":{"step":"not RDF"}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status,matched)=post(app.clone(),"/v1/entities",json!({"types":["urn:ontokiln:lab:SourceField","urn:ontokiln:lab:GoField"],"subclasses":true,"limit":1})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(matched["total"], 7);
    assert_eq!(matched["entities"].as_array().unwrap().len(), 1);
    let (status,batch)=post(app,"/v1/query/batch",json!({"subjects":["urn:missing","urn:ontokiln:lab:FormatAddress"],"predicate":"urn:ontokiln:lab:input","explain":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(batch[0]["results"], json!([]));
    assert_eq!(batch[1]["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        batch[1]["results"][0]["proof"][1]["rule"],
        "inherited_property"
    );
}
