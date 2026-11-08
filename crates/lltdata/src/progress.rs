//! 构建进度回调。
//!
//! 管线本身不打印：CLI 需要能 `-q` 静音、界面需要把进度画进状态栏、Android 需要
//! 把日志丢进 logcat——同一个实现里塞三种输出只会互相打架。

/// 构建过程中的进度。
///
/// 实现方要保证线程安全：`build` 在调用方的后台线程里跑。
pub trait Progress: Send + Sync {
    /// 一条人类可读的进度说明（“解包 xx”“数据版本 xx”）。
    fn note(&self, text: &str);

    /// 下载进度；`total` 为 `None` 表示服务器没给长度。
    ///
    /// 每 512 KB 调一次。
    fn download(&self, _done: u64, _total: Option<u64>) {}
}

/// 什么都不做的进度（`-q` / 测试）。
pub struct Silent;

impl Progress for Silent {
    fn note(&self, _text: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 静音实现不 panic、可跨线程传递（`build` 会在别的线程里用它）。
    #[test]
    fn silent_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Silent>();
        Silent.note("x");
        Silent.download(1, None);
    }
}
