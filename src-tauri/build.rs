use std::path::Path;

const ENV_FILE: &str = "../.env";

fn main() {
    default_sync_server();
    tauri_build::build()
}

/// Builds the app's default sync server in from `HOSTLY_SERVER_URL`: the
/// environment first, then the repository's `.env`. Only that one key is read;
/// nothing else from `.env` ends up in the app.
fn default_sync_server() {
    println!("cargo:rerun-if-env-changed=HOSTLY_SERVER_URL");
    // Watch the file, or the folder until the file exists.
    let watched = if Path::new(ENV_FILE).exists() {
        ENV_FILE
    } else {
        ".."
    };
    println!("cargo:rerun-if-changed={watched}");

    let from_file = || {
        dotenvy::from_path_iter(ENV_FILE)
            .ok()?
            .flatten()
            .find(|(key, _)| key == "HOSTLY_SERVER_URL")
            .map(|(_, value)| value)
    };
    let url = std::env::var("HOSTLY_SERVER_URL").ok().or_else(from_file);
    if let Some(url) = url
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
    {
        println!("cargo:rustc-env=HOSTLY_DEFAULT_SYNC_SERVER={url}");
    }
}
