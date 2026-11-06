fn main() {
    // rust-i18n 的 `i18n!` 在**编译期**把 YAML 读进翻译表：locales/ 改动不会让
    // 本 crate 变脏，增量构建会继续用旧文案（新加键查询不到、直接回吐 key）。
    // 目录级 rerun-if-changed 让 cargo 把 locales/ 的变化认成编译输入。
    println!("cargo:rerun-if-changed=../../locales");
}
