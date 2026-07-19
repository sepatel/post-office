use std::env;
use std::fs;
use std::path::Path;

/// Bake the app's Google OAuth client_id into the binary at compile time by
/// reading `src-tauri/google-oauth.json`. This makes the client_id the default
/// without requiring an env var or a runtime resource file. The source file is
/// git-ignored, so the value lives only in the built binary, not in source.
fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let json_path = Path::new(&manifest)
        .join("..")
        .join("..")
        .join("src-tauri")
        .join("google-oauth.json");

    let read_field = |s: &str, key: &str| -> String {
        serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|v| v.get(key).and_then(|c| c.as_str()).map(|s| s.to_string()))
            .unwrap_or_default()
    };

    let (client_id, client_secret) = fs::read_to_string(&json_path)
        .map(|s| (read_field(&s, "client_id"), read_field(&s, "client_secret")))
        .unwrap_or_default();

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("oauth_client_id.rs");
    fs::write(
        &dest,
        format!(
            "pub const BAKED_GOOGLE_CLIENT_ID: &str = {:?};\npub const BAKED_GOOGLE_CLIENT_SECRET: &str = {:?};\n",
            client_id, client_secret
        ),
    )
    .unwrap();

    println!("cargo:rerun-if-changed={}", json_path.display());
}
