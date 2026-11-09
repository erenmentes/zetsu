fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/zetsu.ico");
        res.set("ProductName", "Zetsu IDE");
        res.set("FileDescription", "Zetsu IDE");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon could not be embedded: {}", e);
        }
    }
}
