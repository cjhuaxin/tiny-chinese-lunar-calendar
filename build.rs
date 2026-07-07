fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile slint ui");

    #[cfg(target_os = "macos")]
    sparklers_build::emit_rpath();
}
