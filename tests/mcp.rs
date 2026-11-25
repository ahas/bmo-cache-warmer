//! The harness plugin's MCP server (docs/api.md, "The harness plugin"): its tools, each called through the
//! client against the service on a loopback port, and the binary spoken to over standard input and output as
//! a harness speaks to it.

use crate::fixtures;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use bmo_cache_warmer::client::{ClientFailure, WarmerClient};
use bmo_cache_warmer::mcp::{ToolFailure, tools};
use bmo_cache_warmer::routes::router;
use fixtures::{NOW, warmer};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const TOKEN: &str = "a-test-token";

const NAMES: [&str; 6] = [
  "warmer_consumers",
  "warmer_targets",
  "warmer_target_put",
  "warmer_target_used",
  "warmer_subjects",
  "warmer_round",
];

/// Calls one tool by name through a client of a served warmer holding one consumer.
async fn called(calls: Vec<(&'static str, Value)>) -> Vec<Result<Value, ToolFailure>> {
  let (_dir, warmer, _clock) = warmer().await;
  warmer.store().put_consumer("alpha", "http://[::1]:1/ping", NOW).await.unwrap();
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = format!("http://{}", listener.local_addr().unwrap());
  let app = router(warmer, TOKEN);
  tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
  let client = WarmerClient::new(&address, TOKEN).unwrap();
  let tools = tools();

  let mut answers = Vec::new();
  for (name, arguments) in calls {
    let tool = tools.iter().find(|tool| tool.name == name).expect("the tool stands");
    answers.push(tool.call(&client, arguments).await);
  }
  answers
}

#[tokio::test]
async fn each_tool_answers_its_routes_member() {
  let answers = called(vec![
    ("warmer_consumers", json!({})),
    (
      "warmer_target_put",
      json!({ "consumerName": "alpha", "targetName": "line", "ttlMs": 3_600_000, "warmByDefault": true }),
    ),
    ("warmer_target_used", json!({ "consumerName": "alpha", "targetName": "line" })),
    ("warmer_targets", json!({ "consumerName": "alpha", "limit": 10 })),
    ("warmer_subjects", json!({ "consumerName": "alpha" })),
    ("warmer_round", json!({})),
  ])
  .await;
  let answers: Vec<Value> = answers.into_iter().map(|answer| answer.expect("the tool answers")).collect();

  assert_eq!(answers[0]["items"][0]["name"], "alpha");
  assert_eq!(
    (answers[1]["name"].clone(), answers[1]["idleMs"].clone()),
    (json!("line"), json!(2_376_000))
  );
  assert_eq!(
    answers[2],
    json!({ "consumerName": "alpha", "targetName": "line", "at": NOW, "startedAt": NOW })
  );
  assert_eq!(
    (answers[3]["items"][0]["usedAt"].clone(), answers[3]["more"].clone()),
    (json!(NOW), json!(false))
  );
  assert_eq!(answers[4]["items"][0]["name"], "line");
  assert_eq!(
    (answers[5]["intervalMs"].clone(), answers[5]["running"].clone()),
    (json!(60_000), json!(true))
  );
}

#[tokio::test]
async fn a_refusal_and_arguments_out_of_shape_come_back_as_tool_errors() {
  let answers = called(vec![
    ("warmer_subjects", json!({ "consumerName": "nobody" })),
    ("warmer_target_put", json!({ "consumerName": "alpha", "targetName": "line" })),
    ("warmer_target_used", json!({ "consumerName": "alpha" })),
    ("warmer_round", json!({ "extra": true })),
  ])
  .await;

  let refused = answers[0].clone().expect_err("no such consumer");
  assert!(matches!(&refused, ToolFailure::Client(ClientFailure::Answered { status: 404, .. })));
  assert_eq!(refused.content()["code"], "not-found");
  assert_eq!(refused.content()["status"], 404);
  for wrong in &answers[1..] {
    let failure = wrong.clone().expect_err("arguments out of shape");
    assert!(matches!(failure, ToolFailure::Arguments(_)));
    assert_eq!(failure.content()["code"], "bad-request");
  }
}

#[tokio::test]
async fn a_service_nobody_listens_on_is_an_error_with_no_code() {
  let closed = std::net::TcpListener::bind("[::1]:0").unwrap();
  let address = format!("http://{}", closed.local_addr().unwrap());
  drop(closed);
  let client = WarmerClient::new(&address, TOKEN).unwrap();
  let failure = tools()[5].call(&client, json!({})).await.expect_err("nothing answers");

  assert!(matches!(failure, ToolFailure::Client(ClientFailure::Unreached(_))));
  assert!(failure.content().get("code").is_none());
}

/// The binary started as `mcp` with these arguments, spoken to over pipes.
fn started(arguments: &[&str]) -> Child {
  Command::new(env!("CARGO_BIN_EXE_bmo-cache-warmer"))
    .arg("mcp")
    .args(arguments)
    .env_remove("CACHE_WARMER_TOKEN")
    .env_remove("CACHE_WARMER_TOKEN_FILE")
    .env_remove("CACHE_WARMER_ADDRESS")
    .env_remove("CACHE_WARMER_SETTINGS")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("the binary starts")
}

/// Waits a bounded time for a child to end, and kills it where it has not.
fn ended(child: &mut Child) -> Option<i32> {
  let deadline = Instant::now() + Duration::from_secs(10);
  while Instant::now() < deadline {
    if let Some(status) = child.try_wait().expect("the child's state reads") {
      return status.code();
    }
    std::thread::sleep(Duration::from_millis(20));
  }
  child.kill().expect("the child is killed");
  child.wait().expect("the child ends");
  None
}

#[test]
fn the_server_lists_exactly_the_six_tools_to_a_harness() {
  let mut child = started(&["--token", "t", "--address", "http://[::1]:1"]);
  let mut input = child.stdin.take().unwrap();
  let mut output = BufReader::new(child.stdout.take().unwrap());
  let say = |input: &mut std::process::ChildStdin, message: Value| {
    writeln!(input, "{message}").and_then(|()| input.flush()).expect("the server reads")
  };
  let mut hear = || {
    let mut line = String::new();
    output.read_line(&mut line).expect("the server answers");
    serde_json::from_str::<Value>(&line).expect("an answer is JSON")
  };

  say(
    &mut input,
    json!({
      "jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "tester", "version": "1" } },
    }),
  );
  let initialized = hear();
  assert_eq!(initialized["result"]["serverInfo"]["name"], "bmo-cache-warmer");
  assert_eq!(initialized["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
  say(&mut input, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
  say(&mut input, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
  let listed = hear();
  let tools = listed["result"]["tools"].as_array().expect("a list of tools");
  assert_eq!(tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect::<Vec<_>>(), NAMES);
  assert!(
    tools
      .iter()
      .all(|tool| tool["inputSchema"]["type"] == "object" && tool["outputSchema"].is_object())
  );

  drop(input);
  assert_eq!(ended(&mut child), Some(0));
}

#[test]
fn the_server_does_not_start_without_a_token() {
  let mut child = started(&["--address", "http://[::1]:1"]);
  drop(child.stdin.take());

  assert_eq!(ended(&mut child), Some(1));
  let mut said = String::new();
  std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut said).unwrap();
  assert!(said.starts_with("No token is given"), "{said}");
}

#[test]
fn the_server_does_not_start_without_an_address() {
  let mut child = started(&["--token", "t"]);
  drop(child.stdin.take());

  assert_eq!(ended(&mut child), Some(1));
  let mut said = String::new();
  std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut said).unwrap();
  assert!(said.starts_with("No address is given"), "{said}");
}
