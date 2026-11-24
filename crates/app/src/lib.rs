//! conix 二进制的全部逻辑。`main.rs` 只解析命令行、接上真实终端；这里的 `Core` 不碰终端，
//! 测试用 `FakeTerm` 与 mock 后端驱动它（docs/design/testing.md §2）。

pub mod config;
pub mod core;
pub mod snapshot;
pub mod term;

pub use core::Core;

/// 把文字放进终端剪贴板的 OSC 52 序列。tmux 里要包一层透传（还要 tmux 开着 `set-clipboard`），
/// 不然它会吃掉这个序列。
pub fn osc52(text: &str) -> String {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod clip_tests {
    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("你好".as_bytes()), "5L2g5aW9");
    }
}
