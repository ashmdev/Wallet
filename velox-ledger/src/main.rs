use std::net::SocketAddr;
use std::sync::Arc;

use tonic::transport::Server;
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;

use velox_ledger::config::Config;
use velox_ledger::proto::ledger_service_server::LedgerServiceServer;
use velox_ledger::repository;
use velox_ledger::service::LedgerServiceImpl;
use velox_ledger::webhook::WebhookDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(Level::INFO.into())
                .add_directive("sqlx::query=warn".parse().unwrap()),
        )
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .json()
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting VeloxLedger service"
    );

    // Load configuration
    let config = Config::from_env().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to load config from env, using defaults");
        Config {
            server: Default::default(),
            database: Default::default(),
            webhook: Default::default(),
        }
    });

    info!(
        host = %config.server.host,
        port = %config.server.port,
        "Configuration loaded"
    );

    // Initialize database pool
    let pool = repository::create_pool(&config.database).await?;
    info!("Database connection pool initialized");

    // Run migrations (in production, you might want to skip this)
    if std::env::var("VELOX__RUN_MIGRATIONS").unwrap_or_default() == "true" {
        info!("Running database migrations...");
        repository::run_migrations(&pool).await?;
        info!("Migrations completed");
    }

    // Initialize webhook dispatcher
    let webhook = Arc::new(WebhookDispatcher::new(config.webhook.clone()));
    info!(
        enabled = config.webhook.enabled,
        "Webhook dispatcher initialized"
    );

    // Create gRPC service
    let ledger_service = LedgerServiceImpl::new(pool.clone(), webhook);

    // Configure and start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    info!(address = %addr, "Starting gRPC server");

    Server::builder()
        .add_service(
            LedgerServiceServer::new(ledger_service)
                .max_decoding_message_size(config.server.grpc_max_message_size)
                .max_encoding_message_size(config.server.grpc_max_message_size),
        )
        .serve_with_shutdown(addr, shutdown_signal())
        .await?;

    info!("Server stopped gracefully");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, initiating graceful shutdown");
        },
        _ = terminate => {
            info!("Received SIGTERM, initiating graceful shutdown");
        },
    }
}
