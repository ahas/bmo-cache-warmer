//! `bmo-cache-warmer mcp`: the harness plugin's MCP server on standard input and output (docs/api.md, "The
//! harness plugin"). Each tool is a thin call of one route through the service's client; the server lists
//! them and the OpenAPI document states them under `x-tools`, both from the one table here.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use rmcp::model::{
  CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, InitializeResult,
  JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use schemars::generate::{SchemaGenerator, SchemaSettings};
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::client::{ClientFailure, WarmerClient};
use crate::wire::{ListQuery, TargetBody, TargetQuery, UseBody};
use crate::words::is_name;

/// The plugin a harness takes the warmer in, and the command that starts its MCP server.
pub const PLUGIN: &str = "bmo-cache-warmer";
pub const MCP: &str = "bmo-cache-warmer mcp";

/// What a harness does first with it, which the server tells a harness when it starts.
pub const ENTRY: &str = "A harness registers its long prefix as a target with warmer_target_put and marks it \
  used after each request with warmer_target_used, so the warmer pings it before the cache runs out.";

/// What a tool's call came to short of the route's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolFailure {
  /// The arguments are not the shape the tool takes.
  Arguments(String),
  /// The call reached no answer of the route's shape.
  Client(ClientFailure),
}

impl ToolFailure {
  /// The failure as the tool error's content: the error shape the service answered with and its status, or the
  /// sentence saying why nothing answered.
  pub fn content(&self) -> Value {
    match self {
      Self::Arguments(why) => json!({ "code": "bad-request", "error": why }),
      Self::Client(ClientFailure::Answered {
        status,
        answer: Some(answer),
        ..
      }) => json!({ "status": status, "code": answer.code, "error": answer.error }),
      Self::Client(failure @ ClientFailure::Answered { status, .. }) => {
        json!({ "status": status, "error": failure.to_string() })
      }
      Self::Client(failure) => json!({ "error": failure.to_string() }),
    }
  }
}

type Called = BoxFuture<'static, Result<Value, ToolFailure>>;
type Call = Box<dyn Fn(WarmerClient, Value) -> Called + Send + Sync>;

/// One tool: its name, what it does, the route it calls, its input and output, and the call itself.
pub struct WarmerTool {
  pub name: &'static str,
  pub description: &'static str,
  /// The route the tool calls, as its method and path.
  pub route: &'static str,
  /// The names the route's path takes, first in the input.
  keys: &'static [&'static str],
  input: fn(&mut SchemaGenerator) -> Schema,
  output: fn(&mut SchemaGenerator) -> Schema,
  call: Call,
}

impl WarmerTool {
  /// The tool's input: the path's names, then the fields of the query or the body.
  pub fn input(&self, generator: &mut SchemaGenerator) -> Schema {
    let rest = (self.input)(generator).to_value();
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();

    for key in self.keys {
      properties.insert((*key).to_owned(), json!({ "type": "string" }));
      required.push(Value::from(*key));
    }
    if let Some(Value::Object(fields)) = rest.get("properties") {
      properties.extend(fields.clone());
    }
    if let Some(Value::Array(names)) = rest.get("required") {
      required.extend(names.iter().cloned());
    }

    let mut object = Map::new();
    if let Some(description) = rest.get("description") {
      object.insert("description".into(), description.clone());
    }
    object.insert("type".into(), "object".into());
    object.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
      object.insert("required".into(), Value::Array(required));
    }
    if let Some(closed) = rest.get("additionalProperties") {
      object.insert("additionalProperties".into(), closed.clone());
    }
    Schema::try_from(Value::Object(object)).expect("an object is a schema")
  }

  /// The tool's output, referred to where `generator` keeps its definitions.
  pub fn output(&self, generator: &mut SchemaGenerator) -> Schema {
    (self.output)(generator)
  }

  /// Calls the tool's route through `client` with the tool's arguments.
  pub async fn call(&self, client: &WarmerClient, arguments: Value) -> Result<Value, ToolFailure> {
    (self.call)(client.clone(), arguments).await
  }
}

/// The path's names taken out of the arguments, each a name, and the rest read as `I`.
fn split<I: DeserializeOwned>(keys: &[&str], arguments: Value) -> Result<(Vec<String>, I), ToolFailure> {
  let wrong = |why: String| {
    ToolFailure::Arguments(format!("The arguments are not the shape this tool takes: {why}."))
  };
  let Value::Object(mut fields) = arguments else {
    return Err(wrong("they are not an object".to_owned()));
  };
  let mut names = Vec::new();

  for key in keys {
    match fields.remove(*key) {
      Some(Value::String(name)) if is_name(&name) => names.push(name),
      Some(_) => return Err(wrong(format!("{key} is not a name of 1 to 256 bytes"))),
      None => return Err(wrong(format!("{key} is missing"))),
    }
  }
  let rest = serde_json::from_value(Value::Object(fields)).map_err(|why| wrong(why.to_string()))?;

  Ok((names, rest))
}

/// A tool whose path takes `keys`, whose rest of the input is `I` and whose answer is `O`.
fn tool<I, O, F, Fut>(
  name: &'static str,
  description: &'static str,
  route: &'static str,
  keys: &'static [&'static str],
  call: F,
) -> WarmerTool
where
  I: DeserializeOwned + JsonSchema + Send + 'static,
  O: Serialize + JsonSchema,
  F: Fn(WarmerClient, Vec<String>, I) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<O, ClientFailure>> + Send + 'static,
{
  let call = Arc::new(call);

  WarmerTool {
    name,
    description,
    route,
    keys,
    input: I::json_schema,
    output: SchemaGenerator::subschema_for::<O>,
    call: Box::new(move |client, arguments| {
      let call = Arc::clone(&call);
      Box::pin(async move {
        let (names, input) = split::<I>(keys, arguments)?;
        let answer = call(client, names, input).await.map_err(ToolFailure::Client)?;

        serde_json::to_value(answer).map_err(|why| ToolFailure::Arguments(why.to_string()))
      })
    }),
  }
}

/// No input beyond the path's names.
#[derive(serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Nothing {}

/// The six tools.
pub fn tools() -> Vec<WarmerTool> {
  vec![
    tool(
      "warmer_consumers",
      "Every consumer.",
      "GET /consumers",
      &[],
      |client, _, query: ListQuery| async move { client.consumers(&query).await },
    ),
    tool(
      "warmer_targets",
      "Every target and when it is pinged next.",
      "GET /targets",
      &[],
      |client, _, query: TargetQuery| async move { client.targets(&query).await },
    ),
    tool(
      "warmer_target_put",
      "Register a target to keep warm.",
      "PUT /consumers/{consumerName}/targets/{targetName}",
      &["consumerName", "targetName"],
      |client, names, body: TargetBody| async move { client.put_target(&names[0], &names[1], &body).await },
    ),
    tool(
      "warmer_target_used",
      "Record that a target was used just now.",
      "POST /consumers/{consumerName}/targets/{targetName}/uses",
      &["consumerName", "targetName"],
      |client, names, body: UseBody| async move { client.record_use(&names[0], &names[1], &body).await },
    ),
    tool(
      "warmer_subjects",
      "A consumer's subjects and whether each is on.",
      "GET /consumers/{consumerName}/subjects",
      &["consumerName"],
      |client, names, query: ListQuery| async move { client.subjects(&names[0], &query).await },
    ),
    tool(
      "warmer_round",
      "The warming round: its interval and next time.",
      "GET /round",
      &[],
      |client, _, _: Nothing| async move { client.round().await },
    ),
  ]
}

/// The generator every tool schema and the OpenAPI document are written with: JSON Schema 2020-12, read as a
/// caller sends it, definitions kept at `definitions_path`.
pub fn generator(definitions_path: &'static str) -> SchemaGenerator {
  SchemaSettings::draft2020_12()
    .for_deserialize()
    .with(|settings| {
      settings.definitions_path = definitions_path.into();
      settings.meta_schema = None;
    })
    .into_generator()
}

/// The server over the service the client reaches.
pub struct WarmerServer {
  client: WarmerClient,
  tools: Arc<Vec<WarmerTool>>,
}

impl WarmerServer {
  pub fn new(client: WarmerClient) -> Self {
    Self {
      client,
      tools: Arc::new(tools()),
    }
  }

  /// The tools as MCP lists them, each input and output a schema of its own with its definitions inside.
  pub fn listed(&self) -> Vec<Tool> {
    self.tools.iter().map(listed).collect()
  }
}

fn listed(tool: &WarmerTool) -> Tool {
  let mut generator = generator("/$defs");
  let input = tool.input(&mut generator);
  let output = tool.output(&mut generator);
  let definitions = generator.take_definitions(true);
  let mut listed = Tool::new(tool.name, tool.description, standing_alone(input, &definitions));

  listed.output_schema = Some(Arc::new(standing_alone(output, &definitions)));
  listed
}

/// A schema made to stand on its own: an object at its root, with every definition it refers to beside it.
fn standing_alone(schema: Schema, definitions: &Map<String, Value>) -> JsonObject {
  let mut object = match schema.to_value() {
    Value::Object(object) if object.contains_key("$ref") => {
      let mut wrapped = Map::new();

      wrapped.insert("allOf".into(), Value::Array(vec![Value::Object(object)]));
      wrapped.insert("type".into(), "object".into());
      wrapped
    }
    Value::Object(object) => object,
    _ => Map::from_iter([("type".to_owned(), Value::from("object"))]),
  };

  if !definitions.is_empty() {
    object.insert("$defs".into(), Value::Object(definitions.clone()));
  }

  object
}

impl ServerHandler for WarmerServer {
  fn get_info(&self) -> InitializeResult {
    let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
    let mut server = Implementation::default();

    server.name = "bmo-cache-warmer".to_owned();
    server.version = crate::VERSION.to_owned();
    info.server_info = server;
    info.instructions = Some(ENTRY.to_owned());
    info
  }

  async fn list_tools(
    &self,
    _: Option<PaginatedRequestParams>,
    _: RequestContext<RoleServer>,
  ) -> Result<ListToolsResult, ErrorData> {
    Ok(ListToolsResult::with_all_items(self.listed()))
  }

  async fn call_tool(
    &self,
    request: CallToolRequestParams,
    _: RequestContext<RoleServer>,
  ) -> Result<CallToolResponse, ErrorData> {
    let Some(tool) = self.tools.iter().find(|tool| tool.name == request.name) else {
      return Err(ErrorData::invalid_params(format!("There is no tool {}.", request.name), None));
    };
    let arguments = Value::Object(request.arguments.unwrap_or_default());
    let result = match tool.call(&self.client, arguments).await {
      Ok(answer) => CallToolResult::structured(answer),
      Err(failure) => CallToolResult::error(vec![ContentBlock::text(failure.content().to_string())]),
    };

    Ok(CallToolResponse::Complete(result))
  }
}
