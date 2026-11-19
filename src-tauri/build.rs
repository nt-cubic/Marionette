fn main() {
    // Tauri CLI 没有 --no-default-features。Rust is the default Cargo feature,
    // but the embedded Magpie fallback must be able to override it without
    // relying on a CLI flag Tauri does not expose. Derive one internal cfg so
    // every `engine-rust` code path agrees on the selected engine.
    println!("cargo:rustc-check-cfg=cfg(engine_rust_active)");
    println!("cargo:rerun-if-env-changed=MARIONETTE_ENGINE");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_ENGINE_RUST");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_EMBEDDED_ENGINE");
    let rust_feature = std::env::var_os("CARGO_FEATURE_ENGINE_RUST").is_some();
    let embedded = std::env::var_os("CARGO_FEATURE_EMBEDDED_ENGINE").is_some();
    let explicit_magpie = std::env::var("MARIONETTE_ENGINE")
        .map(|v| v.eq_ignore_ascii_case("magpie"))
        .unwrap_or(false);
    if rust_feature && !embedded && !explicit_magpie {
        println!("cargo:rustc-cfg=engine_rust_active");
    }
    tauri_build::build()
}
