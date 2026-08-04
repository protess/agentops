//! Environment variable configuration. **Defaults lean safe** — with no authentication in
//! v0.1, the default bind address is the security boundary (spec Section 13).

use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub anthropic_api_key: String,
    /// Reclaims `running` investigations whose `updated_at` has not advanced for longer than this.
    pub watchdog_idle: Duration,
    /// The watchdog sweep interval.
    pub watchdog_interval: Duration,
    /// How long graceful shutdown waits on tasks (spec Section 6.1, shutdown stage 4).
    pub shutdown_deadline: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind: SocketAddr = std::env::var("AGENTOPS_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
            .parse()?;
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;
        let anthropic_api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY is not set"))?;
        Ok(Self {
            bind,
            database_url,
            anthropic_api_key,
            watchdog_idle: Duration::from_secs(15 * 60),
            watchdog_interval: Duration::from_secs(60),
            shutdown_deadline: Duration::from_secs(30),
        })
    }
}
