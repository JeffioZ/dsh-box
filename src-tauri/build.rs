fn main() {
    // tauri-build 嵌入 exe 图标资源时未声明 rerun-if-changed，
    // 只改图标文件不会触发重链。这里补上跟踪，保证 `npm run icons` 后重新构建即生效。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/favicon-source.svg");
    tauri_build::build()
}
