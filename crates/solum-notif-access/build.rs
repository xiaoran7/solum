fn main() {
    // No JS-invokable commands (Rust calls the Kotlin plugin directly via
    // `run_mobile_plugin`, see src/mobile.rs) — the command list only feeds
    // JS binding generation, which this plugin doesn't need.
    let result = tauri_plugin::Builder::new(&[])
        .android_path("android")
        .try_build();

    if !(cfg!(docsrs) && std::env::var("TARGET").unwrap().contains("android")) {
        result.unwrap();
    }
}
