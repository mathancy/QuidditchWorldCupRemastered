//! The program's icon in its `.exe` on Windows: the logo (`qwc-style/assets/qwc.ico`), for Explorer, the taskbar and
//! shortcuts. A build without the Windows SDK's resource compiler warns and goes on without it.

fn main() {
    println!("cargo:rerun-if-changed=../qwc-style/assets/qwc.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../qwc-style/assets/qwc.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no icon in the exe: {e}");
        }
    }
}
