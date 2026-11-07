fn main() {
    slint_build::compile("ui/app.slint").unwrap();
    // Icône du .exe sous Windows
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().unwrap();
    }
}
