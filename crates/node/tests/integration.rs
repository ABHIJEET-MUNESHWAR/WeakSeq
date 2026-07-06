//! HTTP integration tests for the WeakSeq node.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use weakseq_node::{build_router, build_sequencer, NodeConfig};

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn health_endpoint_ok() {
    let router = build_router(build_sequencer(&NodeConfig::default()));
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");
}

#[tokio::test]
async fn graphql_submit_seal_query_flow() {
    let router = build_router(build_sequencer(&NodeConfig::default()));

    let submit = |q: &str| {
        let body = serde_json::json!({ "query": q }).to_string();
        Request::builder()
            .method("POST")
            .uri("/graphql")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    };

    let resp = router
        .clone()
        .oneshot(submit(
            "mutation { submitOrder(side: BUY, price: 100, quantity: 5) }",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = router
        .clone()
        .oneshot(submit(
            "mutation { submitOrder(side: SELL, price: 90, quantity: 5) }",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = router
        .clone()
        .oneshot(submit(
            "mutation { sealBatch { batchId matchedQuantity attestors } }",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("matchedQuantity"));

    let resp = router
        .oneshot(submit(
            "{ status { healthy validatorCount } latestBatch { batchId } }",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("healthy"));
}
