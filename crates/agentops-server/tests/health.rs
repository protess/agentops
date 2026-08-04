use agentops_server::config::Config;
use std::sync::Mutex;

/// `std::env::set_var` and `remove_var` are process-global state. cargo test runs tests
/// in one binary across several threads by default (even with `--test-threads=4`, two
/// tests still start together when there are only two), so without this lock the two
/// tests trample each other's environment variables and fail intermittently —
/// measured: `missing_api_key_is_a_startup_error` failed in 3 of 5 runs.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Configuration comes from environment variables. The default bind must be
/// `127.0.0.1:3000` — spec Section 13 has no authentication in v0.1, so binding to
/// loopback is the only defense. A default of `0.0.0.0` would open an unauthenticated server to the network.
#[test]
fn test_1_default_bind_is_loopback_only() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: environment variables are touched only while holding the lock.
    unsafe {
        std::env::set_var("DATABASE_URL", "postgres://x/y");
        std::env::set_var("ANTHROPIC_API_KEY", "k");
        std::env::remove_var("AGENTOPS_BIND");
    }
    let c = Config::from_env().expect("it must load from defaults alone");
    assert_eq!(
        c.bind.ip().to_string(),
        "127.0.0.1",
        "v0.1 has no authentication and must not open beyond loopback (spec Section 13)"
    );
    assert_eq!(c.bind.port(), 3000);
}

/// It does not start without the secret. Proceeding quietly with an empty string would
/// surface only as a 401 on the first LLM call.
#[test]
fn missing_api_key_is_a_startup_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: environment variables are touched only while holding the lock.
    unsafe {
        std::env::set_var("DATABASE_URL", "postgres://x/y");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
    assert!(
        Config::from_env().is_err(),
        "a missing ANTHROPIC_API_KEY must be a startup failure"
    );
}
