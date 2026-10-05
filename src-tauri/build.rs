use std::{env, fs, path::PathBuf};

fn main() {
    const KEYS: [&str; 2] = ["NINI_BAIDU_APP_KEY", "NINI_BAIDU_SECRET_KEY"];
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let credentials = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("../.env.baidu.local");
    println!("cargo:rerun-if-changed={}", credentials.display());
    if let Ok(contents) = fs::read_to_string(credentials) {
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if KEYS.contains(&key) && env::var_os(key).is_none() {
                println!("cargo:rustc-env={key}={}", value.trim());
            }
        }
    }
    tauri_build::build()
}
