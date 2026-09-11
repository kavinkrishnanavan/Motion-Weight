//! Stamps the Windows executable with the application mark, so the file in
//! Explorer, the taskbar button and Alt-Tab all carry the logo rather than the
//! default blank program icon.
//!
//! `assets/icon.ico` is generated from the project's `logo.png`. Windows wants a
//! real multi-resolution .ico here, so it is a checked-in build input rather
//! than something derived at compile time: if `logo.png` is replaced, rebuild
//! the .ico from it as well.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=../logo.png");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "MotionWeight");
    res.set("FileDescription", "MotionWeight video editor");
    if let Err(e) = res.compile() {
        // A missing resource compiler must not stop the build; the app just
        // ships without an embedded icon.
        println!("cargo:warning=icon not embedded: {e}");
    }
}
