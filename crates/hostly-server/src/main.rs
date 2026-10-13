use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_LOG: &str = "hostly_server=info,tower_http=info";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A `.env` file is optional; real environment variables take precedence.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG)),
        )
        .init();

    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set; see .env.example")?;
    let bind = std::env::var("HOSTLY_BIND").unwrap_or_else(|_| DEFAULT_BIND.into());

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    hostly_server::migrate(&pool).await?;

    let listener = TcpListener::bind(&bind).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, hostly_server::app(pool))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Ctrl-C, or SIGTERM from `docker stop` and service managers.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}
