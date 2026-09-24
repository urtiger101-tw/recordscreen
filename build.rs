fn main() {
    println!("cargo:rerun-if-changed=assets/recordscreen.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/recordscreen.ico")
            .compile()
            .expect("could not embed RecordScreen icon");
    }
}
