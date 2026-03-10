use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../.env");
    if let Ok(d) = dotenvy::from_path_iter(Path::new("../.env")) {
        for i in d {
            if let Ok(x) = i {
                println!("cargo:rustc-env={}={}", x.0, x.1);
            }
        }
    }
}