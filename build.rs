// Embeds the application icon into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/pixtwin.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/pixtwin.ico");
        res.set("ProductName", "PixTwin");
        res.set("FileDescription", "PixTwin – find similar images");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows icon: {e}");
        }
    }
}
