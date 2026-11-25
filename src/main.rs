//! The service binary (docs/api.md, "The binary"): with its settings from its arguments, its environment
//! and its settings file, serves the warmer's routes and runs its pass on the
//! round's beat until SIGTERM or SIGINT drains it, or, started with a subcommand, serves the harness
//! plugin's MCP server, asks the service's health, or takes or checks a snapshot of the store.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bmo_cache_warmer::client::WarmerClient;
use bmo_cache_warmer::clock::SystemClock;
use bmo_cache_warmer::mcp::WarmerServer;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::service::{self, Ended};
use bmo_cache_warmer::settings::SettingsFile;
use bmo_cache_warmer::snapshot::{self, Manifest};
use bmo_cache_warmer::store::Store;
use clap::{Args, Parser, Subcommand};
use futures_util::Stream;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

// The settings the supervisor starts the service with: each an argument, or the environment where the
// argument is absent, or the settings file where neither names it. A subcommand stands in for the
// service, and the service's settings are then not asked for.
#[derive(Parser)]
#[command(version, about)]
struct Settings {
  #[command(subcommand)]
  command: Option<Command>,
  #[command(flatten)]
  file: FileArg,
  /// The IP address and port to listen on, on loopback; the settings' `listen` where absent.
  #[arg(long, env = "CACHE_WARMER_LISTEN")]
  listen: Option<SocketAddr>,
  /// The path of the store's SQLite file, created where missing; the settings' `store` where absent.
  #[arg(long, env = "CACHE_WARMER_STORE")]
  store: Option<PathBuf>,
  /// The file holding the bearer token every route takes; the settings' `token_file` where absent.
  #[arg(long, env = "CACHE_WARMER_TOKEN_FILE")]
  token_file: Option<PathBuf>,
}

/// The settings file a command reads.
#[derive(Args)]
struct FileArg {
  /// The settings file; `settings.toml` in the folder above the binary's own where absent.
  #[arg(long = "settings", env = "CACHE_WARMER_SETTINGS")]
  settings: Option<PathBuf>,
}

impl FileArg {
  fn read(&self) -> Result<SettingsFile, String> {
    SettingsFile::find(self.settings.as_deref())
  }
}

/// Where a command reaches the service, and with which token.
#[derive(Args)]
struct Reach {
  #[command(flatten)]
  file: FileArg,
  /// The service's address; `http://` and the settings' `listen` where absent.
  #[arg(long, env = "CACHE_WARMER_ADDRESS")]
  address: Option<String>,
  /// The bearer token itself.
  #[arg(long, env = "CACHE_WARMER_TOKEN", hide_env_values = true)]
  token: Option<String>,
  /// The file holding the bearer token, where no token is given; the settings' `token_file` where absent.
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
    None => match service_settings(settings) {
      Ok((listen, store, token_file)) => serve(listen, store, &token_file).await,
      Err(sentence) => Err(sentence),
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

/// The service's three settings, each from the command line or the environment where it names it, else
/// from the settings file.
fn service_settings(settings: Settings) -> Result<(SocketAddr, PathBuf, PathBuf), String> {
  let file = settings.file.read()?;
  let missing = |what: &str, argument: &str, key: &str| {
    format!("No {what} is given: name it with --{argument}, or as `{key}` in the settings file.")
  };
  let listen = settings
    .listen
    .or(file.listen)
    .ok_or_else(|| missing("listen address", "listen", "listen"))?;
  let store = settings.store.or(file.store).ok_or_else(|| missing("store", "store", "store"))?;
  let token_file = settings
    .token_file
    .or(file.token_file)
    .ok_or_else(|| missing("token file", "token-file", "token_file"))?;
  Ok((listen, store, token_file))
}

/// The client a command reaches the service with.
fn client(reach: Reach) -> Result<WarmerClient, String> {
  let file = reach.file.read()?;
  let address = match (reach.address.filter(|address| !address.is_empty()), file.listen) {
    (Some(address), _) => address,
    (None, Some(listen)) => format!("http://{listen}"),
    (None, None) => {
      return Err(
        "No address is given: name one with --address, or as `listen` in the settings file.".to_owned(),
      );
    }
  };
  let token = match (
    reach.token.map(|token| token.trim().to_owned()).filter(|token| !token.is_empty()),
    reach.token_file.or(file.token_file),
  ) {
    (Some(token), _) => token,
    (None, Some(file)) => token_in(&file)?,
    (None, None) => {
      return Err(
        "No token is given: name one with --token or --token-file, or as `token_file` in the settings file."
          .to_owned(),
      );
    }
  };

  WarmerClient::new(&address, token).map_err(|failure| format!("The address does not serve: {failure}."))
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
