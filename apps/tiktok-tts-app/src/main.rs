mod config;
mod core;
#[cfg(feature = "desktop")]
mod desktop;
mod server;

#[cfg(all(not(feature = "onnx"), not(feature = "onnx-dynamic")))]
compile_error!(
    "Yapper needs a TTS backend: build with `--features onnx` (bundled ONNX Runtime) or \
     `--features onnx-dynamic` (load libonnxruntime at runtime)."
);

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
            server::main();
        } else {
            desktop::run();
        }
    }
    #[cfg(not(feature = "desktop"))]
    {
        server::main();
    }
}
