//! The OpenAPI document (docs/api.md, "The contract"): the one the repository keeps is the one the code
//! writes, at the repository's version, with the routes, events, hook and tools the API states.

use bmo_cache_warmer::openapi::{openapi, openapi_json};

const TOOLS: [&str; 6] = [
  "warmer_consumers",
  "warmer_targets",
  "warmer_target_put",
  "warmer_target_used",
  "warmer_subjects",
  "warmer_round",
];

#[test]
fn the_kept_document_is_the_one_the_code_writes() {
  let kept = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/openapi.json")).unwrap();

  assert!(
    kept == openapi_json(),
    "docs/openapi.json differs from what the code writes; write it again with \
     cargo run --example openapi > docs/openapi.json"
  );
}

#[test]
fn the_document_carries_the_version_the_routes_the_events_the_hook_and_a_route_for_every_tool() {
  let document = openapi();
  assert_eq!(document["openapi"], "3.1.0");
  assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
  assert_eq!(document["x-harness"]["plugin"], "bmo-cache-warmer");
  assert_eq!(document["x-harness"]["mcp"], "bmo-cache-warmer mcp");
  assert_eq!(
    document["x-events"],
    serde_json::json!([
      "target.changed",
      "target.deleted",
      "subject.changed",
      "round.changed",
      "pass.created"
    ])
  );
  assert!(document["webhooks"]["target-ping"]["post"].is_object());

  let paths = document["paths"].as_object().unwrap();
  assert_eq!(paths.len(), 12);
  assert_eq!(paths.values().map(|one| one.as_object().unwrap().len()).sum::<usize>(), 15);

  let tools = document["x-tools"].as_array().unwrap();
  assert_eq!(tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect::<Vec<_>>(), TOOLS);
  for tool in tools {
    let (method, path) = tool["route"].as_str().unwrap().split_once(' ').unwrap();

    assert!(document["paths"][path][method.to_lowercase()].is_object(), "{tool}");
  }
}
