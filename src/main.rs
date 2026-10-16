//! The service binary: serves the warmer's routes and runs its pass on the policy's clock (docs/api.md,
//! "Starting the service").

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use bmo_cache_warmer::clock::SystemClock;
use bmo_cache_warmer::pass::Warmer;
use bmo_cache_warmer::policy::Policy;
use bmo_cache_warmer::routes::router;
use bmo_cache_warmer::store::Store;
use bmo_cache_warmer::ticker::warm;
use clap::Parser;
use tokio::net::TcpListener;

// The settings the supervisor starts the service with: each an argument, or the environment where the
// argument is absent.
#[derive(Parser)]
#[command(version, about)]
struct Settings {
  /// The IP address and port to listen on, on loopback.
  #[arg(long, env = "CACHE_WARMER_LISTEN")]
  listen: SocketAddr,
  /// The path of the store's SQLite file, created where missing.
  #[arg(long, env = "CACHE_WARMER_STORE")]
  store: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
  match serve(Settings::parse()).await {
    Ok(()) => ExitCode::SUCCESS,
    Err(sentence) => {
      eprintln!("{sentence}");
      ExitCode::FAILURE
    }
  }
}

/// Opens the store, binds the port, starts the ticker and serves the routes until serving fails.
async fn serve(settings: Settings) -> Result<(), String> {
  let Settings { listen, store } = settings;
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
