//! Yapper application core shared by the desktop, mobile and server shells.

pub mod android_bridge;
pub mod audio_sink;
mod config;
pub mod core;
#[cfg(feature = "desktop")]
mod desktop;
mod edge;
pub mod models;
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

// Workaround for macOS linker error when linking precompiled ONNX Runtime with CoreML:
// CoreML object files in libort_sys reference `___isPlatformVersionAtLeast`, which is provided
// by clang compiler-rt when targeting macOS >= 13.4. Rustc defaults to macOS 11.0 on aarch64.
// Providing a weak assembly definition guarantees the symbol is always resolved without duplicate symbol errors.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".globl ___isPlatformVersionAtLeast",
    ".weak_definition ___isPlatformVersionAtLeast",
    "___isPlatformVersionAtLeast:",
    "mov w0, #1",
    "ret"
);

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
core::arch::global_asm!(
    ".globl ___isPlatformVersionAtLeast",
    ".weak_definition ___isPlatformVersionAtLeast",
    "___isPlatformVersionAtLeast:",
    "mov eax, 1",
    "ret"
);

