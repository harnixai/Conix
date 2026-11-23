use serde::{Deserialize, Serialize};

/// 毫秒时间戳（Unix 纪元起）或毫秒时长，按字段名区分。
pub type Millis = u64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// 计费口径，与 Agix 的预算、provider 配额一致：未命中缓存的输入 + 缓存写入 + 输出，缓存命中不计。
    /// 作业的护栏、每日额度与作业页上的数都用它，和 `max_tokens` 比的是同一个数。
    pub fn billed(&self) -> u64 {
        self.input + self.output + self.cache_write
    }

    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}
