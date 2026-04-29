use std::{env, path::PathBuf};

fn main() {
    // println!("cargo::rustc-link-arg-tests=-Tembedded-test.x");
    let out = &PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out.display());
}
