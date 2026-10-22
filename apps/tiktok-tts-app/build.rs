use std::fs;
use std::path::Path;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Yapper</title>
<style>body{background:#080a0e;color:#e8ecf3;font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0}
main{max-width:32rem;padding:2rem;border:1px solid #262b36;border-radius:16px;background:#0f1218;line-height:1.6}
code{background:#080a0e;padding:.15rem .4rem;border-radius:6px;color:#3ddc97}</style></head>
<body><main><h1>Yapper</h1>
<p>The desktop UI has not been built yet.</p>
<p>Run <code>npm --prefix ui install</code> and <code>npm --prefix ui run build</code>, then rebuild the app.</p>
</main></body></html>"#;

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=ui/dist/index.html");

    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        // `tauri::generate_context!` embeds the frontend at compile time, so a
        // plain `cargo build` needs at least an index.html to exist.
        let dist = Path::new("ui/dist");
        if !dist.join("index.html").is_file() {
            let _ = fs::create_dir_all(dist);
            let _ = fs::write(dist.join("index.html"), PLACEHOLDER);
        }
        tauri_build::build();
    }
}
