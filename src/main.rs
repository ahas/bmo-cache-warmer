//! The service binary (docs/api.md, "The binary"): serves the warmer's routes and runs its pass on the
//! round's beat until SIGTERM or SIGINT drains it, or, started with a subcommand, serves the harness
//! plugin's MCP server, asks the service's health, or takes or checks a snapshot of the store.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bmo_cache_warmer::client::{DEFAULT_ADDRESS, WarmerClient};
use bmo_cache_warmer::clock::SystemClock;
use bmo_cache_warmer::mcp::WarmerServer;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::service::{self, Ended};
use bmo_cache_warmer::snapshot::{self, Manifest};
use bmo_cache_warmer::store::Store;
use clap::{Args, Parser, Subcommand};
use futures_util::Stream;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

// The settings the supervisor starts the service with: each an argument, or the environment where the
// argument is absent. A subcommand stands in for the service, and the service's settings are then not
// asked for.
#[derive(Parser)]
#[command(version, about, subcommand_negates_reqs = true)]
struct Settings {
  #[command(subcommand)]
  command: Option<Command>,
  /// The IP address and port to listen on, on loopback.
  #[arg(long, env = "CACHE_WARMER_LISTEN", required = true)]
  listen: Option<SocketAddr>,
  /// The path of the store's SQLite file, created where missing.
  #[arg(long, env = "CACHE_WARMER_STORE", required = true)]
  store: Option<PathBuf>,
  /// The file holding the bearer token every route takes.
  #[arg(long, env = "CACHE_WARMER_TOKEN_FILE", required = true)]
  token_file: Option<PathBuf>,
}

/// Where a command reaches the service, and with which token.
#[derive(Args)]
struct Reach {
  /// The service's address.
  #[arg(long, env = "CACHE_WARMER_ADDRESS", default_value = DEFAULT_ADDRESS)]
  address: String,
  /// The bearer token itself.
  #[arg(
    long,
    env = "CACHE_WARMER_TOKEN",
    hide_env_values = true,
    required_unless_present = "token_file"
  )]
  token: Option<String>,
  /// The file holding the bearer token, where no token is given.
  #[arg(long, env = "CACHE_WARMER_TOKEN_FILE")]
  token_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
  /// Serves the harness plugin's MCP server on standard input and output.
  Mcp(Reach),
  /// Asks the service whether it is ready.
  Health(Reach),
  /// Writes a consistent copy of the store and its manifest into a directory.
  Snapshot {
    /// The path of the store's SQLite file to copy.
    #[arg(long, env = "CACHE_WARMER_STORE")]
    store: PathBuf,
    /// The directory to write the copy and its manifest into, made where missing.
    #[arg(long, env = "BMO_BACKUP_SNAPSHOT_DIR")]
    into: PathBuf,
  },
  /// Holds a snapshot in a directory to its manifest.
  Verify {
    /// The directory holding the snapshot.
    #[arg(long, env = "BMO_BACKUP_SNAPSHOT_DIR")]
    from: PathBuf,
  },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
  let settings = Settings::parse();
  let done = match settings.command {
    Some(Command::Mcp(reach)) => mcp(reach).await,
    Some(Command::Health(reach)) => health(reach).await,
    Some(Command::Snapshot { store, into }) => take(&store, &into).await,
    Some(Command::Verify { from }) => verify(&from).await,
    None => match (settings.listen, settings.store, settings.token_file) {
      (Some(listen), Some(store), Some(token_file)) => serve(listen, store, &token_file).await,
      _ => unreachable!("the command line requires every setting where no subcommand is given"),
    },
  };
  match done {
    Ok(()) => ExitCode::SUCCESS,
    Err(sentence) => {
      eprintln!("{sentence}");
      ExitCode::FAILURE
    }
  }
}

/// The token a file holds, its surrounding space trimmed; refused where it does not read or holds none.
fn token_in(file: &Path) -> Result<String, String> {
  let token = std::fs::read_to_string(file)
    .map_err(|error| format!("The token file {} does not read: {error}.", file.display()))?;
  let token = token.trim();

  if token.is_empty() {
    return Err(format!("The token file {} holds no token.", file.display()));
  }
  Ok(token.to_owned())
}

/// The client a command reaches the service with.
fn client(reach: Reach) -> Result<WarmerClient, String> {
  let token = match (
    reach.token.map(|token| token.trim().to_owned()).filter(|token| !token.is_empty()),
    reach.token_file,
  ) {
    (Some(token), _) => token,
    (None, Some(file)) => token_in(&file)?,
    (None, None) => return Err("No token is given: name one with --token or --token-file.".to_owned()),
  };

  WarmerClient::new(&reach.address, token)
    .map_err(|failure| format!("The address does not serve: {failure}."))
}

/// Opens the store, binds the port, starts the ticker and serves the routes until SIGTERM or SIGINT, then
/// drains within the policy's deadline.
async fn serve(listen: SocketAddr, store: PathBuf, token_file: &Path) -> Result<(), String> {
  if !listen.ip().is_loopback() {
    return Err(format!("The listen address {listen} is not on loopback."));
  }
  let signals = signals()?;
  let token = token_in(token_file)?;
  let policy = Policy::builtin().map_err(|error| format!("The built-in policy does not read: {error}."))?;
  let opened = Store::open(&store)
    .await
    .map_err(|error| format!("The store at {} does not open. {error}", store.display()))?;
  let listener = TcpListener::bind(listen)
    .await
    .map_err(|error| format!("The address {listen} cannot be bound: {error}."))?;

  let warmer = Warmer::open(opened, policy, Arc::new(SystemClock))
    .await
    .map_err(|error| format!("The store's round does not read. {error}"))?;
  let warmer = Arc::new(warmer);
  let drain = warmer.policy().drain();
  let ended = service::serve(listener, warmer, &token, signals, drain)
    .await
    .map_err(|error| format!("The service stopped serving: {error}."))?;

  if ended == Ended::Cut {
    eprintln!("The drain was cut with calls or pings still in flight.");
  }
  Ok(())
}

/// Each SIGTERM or SIGINT as it comes: the first stops the service, the next cuts its drain. Both are caught
/// from the moment the stream is made, so a signal that comes while the service is still starting is not
/// lost.
fn signals() -> Result<impl Stream<Item = ()>, String> {
  let catch = |kind: SignalKind| {
    signal(kind).map_err(|error| format!("The stop's signals cannot be caught: {error}."))
  };
  let terminate = catch(SignalKind::terminate())?;
  let interrupt = catch(SignalKind::interrupt())?;

  Ok(futures_util::stream::unfold(
    (terminate, interrupt),
    |(mut terminate, mut interrupt)| async move {
      tokio::select! {
        Some(()) = terminate.recv() => {}
        Some(()) = interrupt.recv() => {}
        else => return None,
      }
      Some(((), (terminate, interrupt)))
    },
  ))
}

/// Serves the MCP server until its harness closes standard input.
async fn mcp(reach: Reach) -> Result<(), String> {
  let running = rmcp::ServiceExt::serve(WarmerServer::new(client(reach)?), rmcp::transport::stdio())
    .await
    .map_err(|why| format!("The MCP session did not start: {why}."))?;

  running
    .waiting()
    .await
    .map(drop)
    .map_err(|why| format!("The MCP session ended in a failure: {why}."))
}

/// Asks the service's health; ready is the only success.
async fn health(reach: Reach) -> Result<(), String> {
  let health = client(reach)?
    .health()
    .await
    .map_err(|failure| format!("The service did not answer: {failure}."))?;

  match (health.ready, health.reason) {
    (true, _) => {
      println!("The service is ready, at version {}.", health.version);
      Ok(())
    }
    (false, reason) => Err(format!(
      "The service is not ready: {}",
      reason.unwrap_or_else(|| "it gives no reason.".to_owned())
    )),
  }
}

async fn take(store: &Path, into: &Path) -> Result<(), String> {
  let manifest = snapshot::take(store, into).await.map_err(|error| error.to_string())?;
  match manifest.store {
    Some(held) => println!(
      "The store at {} is copied into {}, at migration {}.",
      store.display(),
      into.display(),
      held.migration
    ),
    None => println!(
      "No store stands at {}, so {} holds only a manifest saying so.",
      store.display(),
      into.display()
    ),
  }
  Ok(())
}

async fn verify(from: &Path) -> Result<(), String> {
  let Manifest { store } = snapshot::verify(from).await.map_err(|error| error.to_string())?;
  match store {
    Some(held) => println!(
      "The snapshot in {} holds to its manifest, at migration {}.",
      from.display(),
      held.migration
    ),
    None => println!("The snapshot in {} says no store stood, and no copy stands.", from.display()),
  }
  Ok(())
}
