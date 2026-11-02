//! The service's OpenAPI 3.1 document (docs/api.md, "The contract"): every route with its parameters, its
//! body, its answers and the codes it can answer, the bodies' schemas written from the types the routes take
//! and answer, the events, the target-ping hook, and the harness plugin's tools from the table the MCP server
//! lists. The repository keeps it as `docs/openapi.json`, and a test holds the kept document to this one.

use schemars::JsonSchema;
use schemars::generate::SchemaGenerator;
use serde_json::{Map, Value, json};

use crate::mcp::{ENTRY, MCP, PLUGIN, generator, tools};
use crate::wire::{
  Consumer, ConsumerBody, Event, EventQuery, Group, GroupBody, Health, ListQuery, Page, Pass, PassBody,
  PingAnswer, Round, RoundPatch, Subject, SubjectPatch, Target, TargetBody, TargetPing, TargetQuery,
  TargetUse, UseBody, WireError,
};

/// Where the schemas the routes refer to stand in the document.
const SCHEMAS: &str = "/components/schemas";

const DESCRIPTION: &str = "A keepalive scheduler for prompt caches: consumers register targets (cached \
  prefixes), record each use, and are asked to ping a target before its cache runs out. Asking is the \
  target-ping webhook each consumer serves at the URL it registered; the service names no consumer and calls \
  no other service. Bodies are JSON with camelCase keys; times are integers of epoch milliseconds. A list is a \
  page in id order, `after` being the id of the last item read. Every failure is answered in the error shape \
  with its status. The harness plugin's tools, each a thin call of one route, are stated under `x-tools`.";

/// The event kinds the stream carries.
pub const EVENTS: [&str; 5] =
  ["target.changed", "target.deleted", "subject.changed", "round.changed", "pass.created"];

/// Each code a route can answer, its status and what it means.
const CODES: &[(&str, u16, &str)] = &[
  (
    "bad-request",
    400,
    "the body, the query or a name in the path is not the shape the route takes",
  ),
  ("unauthorized", 401, "the bearer token is missing or wrong"),
  ("no-route", 404, "no route answers this method and path"),
  ("not-found", 404, "nothing stands at the names the path gives"),
  ("conflict", 409, "a target still names the group"),
  ("gone", 410, "the events past the cursor asked for are no longer kept"),
  ("invalid-url", 422, "the hook URL does not parse or is not http"),
  ("invalid-clock", 422, "a span or a time is out of its range"),
  ("invalid-kind", 422, "the kind is not a kind word"),
  ("unknown-group", 422, "the registration names no group of the consumer"),
  ("failed", 500, "the store could not be read or written"),
];

/// One route as the document states it.
struct Route {
  method: &'static str,
  path: &'static str,
  id: &'static str,
  summary: &'static str,
  parameters: Vec<Value>,
  body: Option<Value>,
  /// The success's status and content, none for an answer with no body.
  answer: (u16, Option<Value>),
  /// The codes of its own, beside the token's and those every route can answer.
  codes: Vec<&'static str>,
}

impl Route {
  fn operation(self) -> Value {
    let mut codes = vec!["unauthorized"];

    if !self.parameters.is_empty() || self.body.is_some() {
      codes.push("bad-request");
    }

    codes.extend(self.codes);
    codes.push("failed");

    let mut responses = Map::new();
    let (status, content) = self.answer;
    let mut success = json!({ "description": self.summary });

    if let Some(content) = content {
      success["content"] = content;
    }

    // A PUT creates where nothing stood, and then answers 201 with the same member.
    if self.method == "put" {
      responses.insert("201".into(), success.clone());
    }
    responses.insert(status.to_string(), success);
    responses.insert("default".into(), json!({ "$ref": "#/components/responses/Error" }));

    let tag = self.path[1..].split('/').next().unwrap_or_default();
    let mut operation = json!({
      "operationId": self.id,
      "tags": [tag],
      "summary": self.summary,
      "responses": responses,
      "x-codes": codes,
    });

    if !self.parameters.is_empty() {
      operation["parameters"] = self.parameters.into();
    }

    if let Some(body) = self.body {
      operation["requestBody"] =
        json!({ "required": true, "content": { "application/json": { "schema": body } } });
    }

    operation
  }
}

fn json_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Value {
  json!({ "application/json": { "schema": generator.subschema_for::<T>() } })
}

fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Value {
  generator.subschema_for::<T>().to_value()
}

fn key(name: &str, description: &str) -> Value {
  json!({ "name": name, "in": "path", "required": true, "description": description, "schema": { "type": "string" } })
}

fn consumer_key() -> Value {
  key("consumerName", "The consumer's name, percent-encoded.")
}

fn target_key() -> Value {
  key("targetName", "The target's name under its consumer, percent-encoded.")
}

/// The query parameters of a query type, each property one parameter.
fn query<Query: JsonSchema>(generator: &mut SchemaGenerator) -> Vec<Value> {
  let schema = Query::json_schema(generator).to_value();
  let Some(properties) = schema["properties"].as_object() else {
    return Vec::new();
  };

  properties
    .iter()
    .map(|(name, property)| {
      let mut parameter = json!({ "name": name, "in": "query", "required": false, "schema": property });

      if let Some(description) = property.get("description") {
        parameter["description"] = description.clone();
      }

      parameter
    })
    .collect()
}

fn routes(generator: &mut SchemaGenerator) -> Vec<Route> {
  let page_of_events = json!({
    "text/event-stream": { "schema": { "type": "string", "description": "Server-sent events, each an event." } },
    "application/json": { "schema": generator.subschema_for::<Page<Event>>() },
  });

  vec![
    Route {
      method: "get",
      path: "/health",
      id: "readHealth",
      summary: "Readiness and version.",
      parameters: Vec::new(),
      body: None,
      answer: (200, Some(json_of::<Health>(generator))),
      codes: Vec::new(),
    },
    Route {
      method: "get",
      path: "/events",
      id: "listEvents",
      summary: "The service's one event stream (SSE), or a page of it with Accept: application/json.",
      parameters: query::<EventQuery>(generator),
      body: None,
      answer: (200, Some(page_of_events)),
      codes: vec!["gone"],
    },
    Route {
      method: "get",
      path: "/consumers",
      id: "listConsumers",
      summary: "Every consumer.",
      parameters: query::<ListQuery>(generator),
      body: None,
      answer: (200, Some(json_of::<Page<Consumer>>(generator))),
      codes: Vec::new(),
    },
    Route {
      method: "put",
      path: "/consumers/{consumerName}",
      id: "putConsumer",
      summary: "Register a consumer and the URL its target-ping hook is served at.",
      parameters: vec![consumer_key()],
      body: Some(schema_of::<ConsumerBody>(generator)),
      answer: (200, Some(json_of::<Consumer>(generator))),
      codes: vec!["invalid-url"],
    },
    Route {
      method: "put",
      path: "/consumers/{consumerName}/groups/{groupName}",
      id: "putGroup",
      summary: "Declare a group of targets or change its limit.",
      parameters: vec![
        consumer_key(),
        key("groupName", "The group's name under its consumer, percent-encoded."),
      ],
      body: Some(schema_of::<GroupBody>(generator)),
      answer: (200, Some(json_of::<Group>(generator))),
      codes: vec!["not-found"],
    },
    Route {
      method: "delete",
      path: "/consumers/{consumerName}/groups/{groupName}",
      id: "deleteGroup",
      summary: "Remove a group no target names.",
      parameters: vec![
        consumer_key(),
        key("groupName", "The group's name under its consumer, percent-encoded."),
      ],
      body: None,
      answer: (204, None),
      codes: vec!["not-found", "conflict"],
    },
    Route {
      method: "get",
      path: "/targets",
      id: "listTargets",
      summary: "Every target and when it is pinged next.",
      parameters: query::<TargetQuery>(generator),
      body: None,
      answer: (200, Some(json_of::<Page<Target>>(generator))),
      codes: Vec::new(),
    },
    Route {
      method: "put",
      path: "/consumers/{consumerName}/targets/{targetName}",
      id: "putTarget",
      summary: "Register a target to keep warm, or restate one.",
      parameters: vec![consumer_key(), target_key()],
      body: Some(schema_of::<TargetBody>(generator)),
      answer: (200, Some(json_of::<Target>(generator))),
      codes: vec!["not-found", "invalid-clock", "invalid-kind", "unknown-group"],
    },
    Route {
      method: "delete",
      path: "/consumers/{consumerName}/targets/{targetName}",
      id: "deleteTarget",
      summary: "Stop keeping a target warm.",
      parameters: vec![consumer_key(), target_key()],
      body: None,
      answer: (204, None),
      codes: vec!["not-found"],
    },
    Route {
      method: "post",
      path: "/consumers/{consumerName}/targets/{targetName}/uses",
      id: "createUse",
      summary: "Record that a target was used just now.",
      parameters: vec![consumer_key(), target_key()],
      body: Some(schema_of::<UseBody>(generator)),
      answer: (201, Some(json_of::<TargetUse>(generator))),
      codes: vec!["not-found"],
    },
    Route {
      method: "get",
      path: "/consumers/{consumerName}/subjects",
      id: "listSubjects",
      summary: "A consumer's subjects and whether each is on.",
      parameters: [vec![consumer_key()], query::<ListQuery>(generator)].concat(),
      body: None,
      answer: (200, Some(json_of::<Page<Subject>>(generator))),
      codes: vec!["not-found"],
    },
    Route {
      method: "patch",
      path: "/consumers/{consumerName}/subjects/{subjectName}",
      id: "changeSubject",
      summary: "Switch a subject on or off, or pause it until a time.",
      parameters: vec![
        consumer_key(),
        key("subjectName", "The subject's name under its consumer, percent-encoded."),
      ],
      body: Some(schema_of::<SubjectPatch>(generator)),
      answer: (200, Some(json_of::<Subject>(generator))),
      codes: vec!["not-found", "invalid-clock"],
    },
    Route {
      method: "get",
      path: "/round",
      id: "readRound",
      summary: "The warming round: its interval and next time.",
      parameters: Vec::new(),
      body: None,
      answer: (200, Some(json_of::<Round>(generator))),
      codes: Vec::new(),
    },
    Route {
      method: "patch",
      path: "/round",
      id: "changeRound",
      summary: "Change the round's interval, or stop or run every warming at once.",
      parameters: Vec::new(),
      body: Some(schema_of::<RoundPatch>(generator)),
      answer: (200, Some(json_of::<Round>(generator))),
      codes: vec!["invalid-clock"],
    },
    Route {
      method: "post",
      path: "/passes",
      id: "createPass",
      summary: "Run one warming pass now.",
      parameters: Vec::new(),
      body: Some(schema_of::<PassBody>(generator)),
      answer: (201, Some(json_of::<Pass>(generator))),
      codes: vec!["invalid-clock"],
    },
  ]
}

/// The target-ping hook a consumer serves at the URL it registered.
fn webhooks(generator: &mut SchemaGenerator) -> Value {
  json!({
    "target-ping": {
      "post": {
        "summary": "The warmer asks a consumer to ping a target before its cache runs out.",
        "requestBody": { "required": true, "content": json_of::<TargetPing>(generator) },
        "responses": {
          "204": { "description": "The prefix was read and written again: warmed." },
          "200": { "description": "What became of the ping.", "content": json_of::<PingAnswer>(generator) },
        },
      },
    },
  })
}

/// The OpenAPI document.
pub fn openapi() -> Value {
  let mut generator = generator(SCHEMAS);
  let mut paths = Map::new();

  for route in routes(&mut generator) {
    let (path, method) = (route.path, route.method);

    paths.entry(path).or_insert_with(|| json!({}))[method] = route.operation();
  }

  let webhooks = webhooks(&mut generator);
  let x_tools: Vec<Value> = tools()
    .iter()
    .map(|tool| {
      json!({
        "name": tool.name,
        "description": tool.description,
        "input": tool.input(&mut generator),
        "output": tool.output(&mut generator),
        "route": tool.route,
      })
    })
    .collect();
  let error = generator.subschema_for::<WireError>();
  let codes: Vec<String> = CODES
    .iter()
    .map(|(code, status, meaning)| format!("- `{code}` ({status}): {meaning}"))
    .collect();
  let tags: Vec<Value> = ["health", "events", "consumers", "targets", "round", "passes"]
    .iter()
    .map(|name| json!({ "name": name }))
    .collect();

  json!({
    "openapi": "3.1.0",
    "info": { "title": "bmo-cache-warmer", "version": env!("CARGO_PKG_VERSION"), "description": DESCRIPTION },
    "servers": [{ "url": "/", "description": "the cache-warmer service" }],
    "security": [{ "bearer": [] }],
    "x-layer": "upstream",
    "x-stage": 2,
    "x-calls": [],
    "x-harness": { "plugin": PLUGIN, "mcp": MCP, "entry": ENTRY },
    "tags": tags,
    "paths": paths,
    "webhooks": webhooks,
    "components": {
      "schemas": generator.take_definitions(true),
      "responses": {
        "Error": {
          "description": format!("A failure, in the common error shape. The codes:\n{}", codes.join("\n")),
          "content": { "application/json": { "schema": error } },
        },
      },
      "securitySchemes": {
        "bearer": {
          "type": "http",
          "scheme": "bearer",
          "description": "The service's token, from the caller's own configuration.",
        },
      },
    },
    "x-events": EVENTS,
    "x-tools": x_tools,
  })
}

/// The document as the repository keeps it: two-space indented JSON with a closing newline.
pub fn openapi_json() -> String {
  let mut text = serde_json::to_string_pretty(&openapi()).unwrap_or_default();

  text.push('\n');
  text
}
