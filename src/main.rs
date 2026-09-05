use clap::Parser;
use kirara::{AppError, ServerOptions, cli::Cli};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().map_err(|source| AppError::CreateDir {
        path: ".".to_string(),
        source,
    })?;
    let (base_dir, db_dir) = cli.resolve_paths(&cwd);
    let mut server = kirara::start(ServerOptions {
        base_dir,
        db_dir,
        listen: cli.resolve_endpoint()?,
    })
    .await?;
    tokio::select! {
        result = server.wait() => result,
        result = shutdown_signal() => {
            let stopped = server.shutdown().await;
            result?;
            stopped
        }
    }
}

async fn shutdown_signal() -> Result<(), AppError> {
    let signal_error = |error| AppError::Server {
        message: format!("failed to listen for shutdown signal: {error}"),
    };
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(signal_error)?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(signal_error),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.map_err(signal_error)
}
