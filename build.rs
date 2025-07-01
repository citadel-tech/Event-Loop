use std::env;

fn main() {
    let rust_toolchain = env::var("RUSTUP_TOOLCHAIN").unwrap();
    if rust_toolchain.starts_with("nightly") {
        //enable the unstable feature flag
        println!("cargo:rustc-cfg=feature=\"unstable\"");
    }
}
