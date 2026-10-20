//! Embute o `app.manifest` no executável via recurso Win32.
//! `embed-resource` funciona tanto com toolchain MSVC (rc.exe) quanto GNU (windres).

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/brand/icon.ico");
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("falha ao embutir app.manifest");
}
