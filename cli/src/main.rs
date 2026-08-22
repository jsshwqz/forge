//! forge CLI 占位入口。
//!
//! 第一阶段仅输出版本信息后退出。

fn main() {
    println!("forge {}", env!("CARGO_PKG_VERSION"));
}
