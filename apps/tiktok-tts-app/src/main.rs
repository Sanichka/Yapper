use tracing_subscriber::EnvFilter;

fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn,ort=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    #[cfg(feature = "desktop")]
    {
        let server_mode = std::env::args().skip(1).any(|arg| arg == "--server");
        if server_mode {
            tiktok_tts_app::run_server();
        } else {
            tiktok_tts_app::run();
        }
    }
    #[cfg(not(feature = "desktop"))]
    {
        tiktok_tts_app::run_server();
    }
}
