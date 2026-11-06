fn main() {
    // 同 crossh-core：`i18n!` 编译期嵌入 locales/，必须显式声明依赖，
    // 否则改文案后增量构建仍用旧翻译表。
    println!("cargo:rerun-if-changed=../../locales");
}
