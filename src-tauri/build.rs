fn main() {
    // tauri-build 嵌入 exe 图标资源时未声明 rerun-if-changed，
    // 只改图标文件不会触发重链。这里补上跟踪，保证 `npm run icons` 后重新构建即生效。
    // 图标文件来源是 tauri.conf.json 的 bundle.icon（bundle 未激活，但 tauri-build
    // 仍从中取 .ico 嵌入 exe），因此 bundle.icon 列表不能删除。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/favicon-source.svg");
    // tauri-codegen 嵌入 frontendDist（../ui）资源时同样没有跟踪声明：
    // 补上目录级跟踪，改 UI 文件后 release 构建自动重新嵌入资源
    println!("cargo:rerun-if-changed=../ui");
    tauri_build::build()
}
