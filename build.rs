fn main() {
    println!("cargo:rerun-if-changed=assets/recordscreen.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let features = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
        assert!(
            features.split(',').any(|feature| feature == "crt-static"),
            "RecordScreen requires static CRT linkage. Build from the repository root and preserve the +crt-static flag in .cargo/config.toml."
        );
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/recordscreen.ico")
            .compile()
            .expect("could not embed RecordScreen icon");
    }
}
