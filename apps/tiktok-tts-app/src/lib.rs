//! Yapper application core shared by the desktop, mobile and server shells.

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

/// Starts the Tauri shell. On Android and iOS this is the JNI entry point.
#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    desktop::run();
}

/// Starts the headless HTTP/WebSocket server.
pub fn run_server() {
    server::main();
}
