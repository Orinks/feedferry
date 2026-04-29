fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=feedferry.exe.manifest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::env::current_dir()
            .expect("current dir")
            .join("feedferry.exe.manifest");

        println!("cargo:rustc-link-arg-bin=feedferry=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=feedferry=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
