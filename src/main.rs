//! The service binary: serves the warmer's routes and runs its pass on the policy's clock (docs/api.md,
//! "Starting the service"), or, started with a subcommand, takes or checks a snapshot of the store
//! (docs/api.md, "Snapshot and verify").

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bmo_cache_warmer::clock::SystemClock;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::routes::router;
use bmo_cache_warmer::snapshot::{self, Manifest};
use bmo_cache_warmer::store::Store;
use bmo_cache_warmer::ticker::warm;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;

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
}

#[derive(Subcommand)]
enum Command {
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
    Some(Command::Snapshot { store, into }) => take(&store, &into).await,
    Some(Command::Verify { from }) => verify(&from).await,
    None => match (settings.listen, settings.store) {
      (Some(listen), Some(store)) => serve(listen, store).await,
      _ => unreachable!("the command line requires both settings where no subcommand is given"),
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

/// Opens the store, binds the port, starts the ticker and serves the routes until serving fails.
async fn serve(listen: SocketAddr, store: PathBuf) -> Result<(), String> {
  if !listen.ip().is_loopback() {
    return Err(format!("The listen address {listen} is not on loopback."));
  }
  let policy = Policy::builtin().map_err(|error| format!("The built-in policy does not read: {error}."))?;
  let opened = Store::open(&store)
    .await
    .map_err(|error| format!("The store at {} does not open. {error}", store.display()))?;
  let listener = TcpListener::bind(listen)
    .await
    .map_err(|error| format!("The address {listen} cannot be bound: {error}."))?;

  let warmer = Arc::new(Warmer::new(opened, policy, Arc::new(SystemClock)));
  tokio::spawn(warm(warmer.clone()));
  axum::serve(listener, router(warmer))
    .await
    .map_err(|error| format!("The service stopped serving: {error}."))
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
