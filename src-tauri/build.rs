use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let build_time = format!("{}", secs);
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", build_time);
    tauri_build::build()
}
