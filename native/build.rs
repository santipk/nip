fn main() {
    println!("cargo:rerun-if-changed=assets/nip-cat.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winres::WindowsResource::new()
            .set_icon("assets/nip-cat.ico")
            .set("ProductName", "NIP")
            .set("FileDescription", "NIP Native")
            .set("LegalCopyright", "Copyright (c) 2026 Santiago Abregu")
            .set("OriginalFilename", "NIPNative.exe")
            .compile()
            .expect("Could not compile Windows version resources");
    }
}
