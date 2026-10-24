use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=SPEEXDSP_STATIC_LIB_DIR");
    if env::var_os("CARGO_FEATURE_SPEEXDSP_STATIC").is_none() {
        return;
    }
    let directory = env::var_os("SPEEXDSP_STATIC_LIB_DIR")
        .map(PathBuf::from)
        .expect("speexdsp-static requires SPEEXDSP_STATIC_LIB_DIR pointing to official libspeexdsp.a");
    let archive = directory.join("libspeexdsp.a");
    if !archive.is_file() {
        panic!(
            "speexdsp-static requires {}, built from official xiph/speexdsp with -DFIXED_POINT",
            archive.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", directory.display());
    println!("cargo:rustc-link-lib=static=speexdsp");
    println!("cargo:rustc-link-lib=m");
}
