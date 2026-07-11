use std::fs;
use std::path::Path;

fn main() {
    embed_qweather_credentials();

    slint_build::compile("ui/app.slint").expect("failed to compile slint ui");

    #[cfg(target_os = "macos")]
    sparklers_build::emit_rpath();
}

/// Bakes QWeather JWT credentials into the binary at compile time so end users
/// don't need to configure anything.
fn embed_qweather_credentials() {
    println!("cargo:rerun-if-changed=qweather.local.json");
    println!("cargo:rerun-if-changed=qweather.private.pem");
    println!("cargo:rerun-if-env-changed=QWEATHER_API_HOST");
    println!("cargo:rerun-if-env-changed=QWEATHER_KID");
    println!("cargo:rerun-if-env-changed=QWEATHER_PROJECT_ID");
    println!("cargo:rerun-if-env-changed=QWEATHER_PRIVATE_KEY");
    println!("cargo:rerun-if-env-changed=QWEATHER_PRIVATE_KEY_PATH");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let local_path = Path::new(&manifest_dir).join("qweather.local.json");

    let mut host = String::new();
    let mut kid = String::new();
    let mut project_id = String::new();
    let mut private_key_pem = String::new();

    if let Ok(content) = fs::read_to_string(&local_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            host = json
                .get("api_host")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            kid = json
                .get("kid")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            project_id = json
                .get("project_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();

            if let Some(inline) = json.get("private_key").and_then(|value| value.as_str()) {
                private_key_pem = inline.replace("\\n", "\n");
            } else {
                let key_path = json
                    .get("private_key_path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("qweather.private.pem");
                let pem_path = Path::new(&manifest_dir).join(key_path);
                if let Ok(content) = fs::read_to_string(pem_path) {
                    private_key_pem = content;
                }
            }
        }
    }

    if let Ok(value) = std::env::var("QWEATHER_API_HOST") {
        if !value.is_empty() {
            host = value;
        }
    }
    if let Ok(value) = std::env::var("QWEATHER_KID") {
        if !value.is_empty() {
            kid = value;
        }
    }
    if let Ok(value) = std::env::var("QWEATHER_PROJECT_ID") {
        if !value.is_empty() {
            project_id = value;
        }
    }
    if let Ok(value) = std::env::var("QWEATHER_PRIVATE_KEY") {
        if !value.is_empty() {
            private_key_pem = value.replace("\\n", "\n");
        }
    }
    if private_key_pem.is_empty() {
        if let Ok(path) = std::env::var("QWEATHER_PRIVATE_KEY_PATH") {
            if let Ok(content) = fs::read_to_string(path) {
                private_key_pem = content;
            }
        }
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let key_out = Path::new(&out_dir).join("qweather_private_key.pem");
    fs::write(&key_out, private_key_pem.trim()).expect("failed to write QWeather private key");

    println!("cargo:rustc-env=QWEATHER_API_HOST={host}");
    println!("cargo:rustc-env=QWEATHER_KID={kid}");
    println!("cargo:rustc-env=QWEATHER_PROJECT_ID={project_id}");

    if host.is_empty() || kid.is_empty() || project_id.is_empty() || private_key_pem.trim().is_empty()
    {
        println!(
            "cargo:warning=QWeather JWT credentials missing. \
             Copy qweather.local.example.json to qweather.local.json, generate an Ed25519 key pair, \
             upload the public key to the QWeather console, and save the private key as qweather.private.pem."
        );
    }
}
