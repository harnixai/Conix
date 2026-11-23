//! 无人值守的护栏（architecture §8.5）：全局缺省与天花板、作业自己写的项、实际生效的值。
//! 天花板来自这台机器的 `codix.yaml`，作业定义来自仓库：仓库里的作业只能比天花板更紧。

use port::{JobGuard, Network, Overlap};

const MIN: u64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub struct GuardConfig {
    /// 作业没写时用；每项都有值。
    pub defaults: JobGuard,
    pub max_timeout_ms: u64,
    pub max_tokens: u64,
    /// 这个实例所有无人值守的工作一天合计；0 是不限。
    pub daily_tokens: u64,
    pub max_concurrent: u32,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            defaults: JobGuard {
                timeout_ms: Some(30 * MIN),
                max_tokens: Some(2_000_000),
                overlap: Some(Overlap::Skip),
                disable_after: Some(3),
                network: Some(Network::DenyAll),
            },
            max_timeout_ms: 240 * MIN,
            max_tokens: 20_000_000,
            daily_tokens: 50_000_000,
            max_concurrent: 2,
        }
    }
}

impl GuardConfig {
    /// 作业写的项取与天花板的较小值，没写的用缺省（缺省同样不超过天花板）。返回生效值与被压过的项名。
    pub fn effective(&self, g: &JobGuard) -> (JobGuard, Vec<String>) {
        let mut capped = vec![];
        let mut cap = |name: &str, want: Option<u64>, default: Option<u64>, max: u64| {
            let v = want.or(default).unwrap_or(max);
            if v > max {
                if want.is_some() {
                    capped.push(name.to_string());
                }
                max
            } else {
                v
            }
        };
        let timeout = cap(
            "timeout",
            g.timeout_ms,
            self.defaults.timeout_ms,
            self.max_timeout_ms,
        );
        let tokens = cap(
            "max_tokens",
            g.max_tokens,
            self.defaults.max_tokens,
            self.max_tokens,
        );
        (
            JobGuard {
                timeout_ms: Some(timeout),
                max_tokens: Some(tokens),
                overlap: g.overlap.or(self.defaults.overlap).or(Some(Overlap::Skip)),
                disable_after: g.disable_after.or(self.defaults.disable_after).or(Some(0)),
                // 网络没有天花板：放开网络是作业作者与这台机器的用户各自的决定，Agix 那边还有
                // `sandbox.unattended_needs_network_policy` 守着（inherit 下没有 allow 规则的命令照样被拒）。
                network: g
                    .network
                    .or(self.defaults.network)
                    .or(Some(Network::DenyAll)),
            },
            capped,
        )
    }
}

/// `codix.yaml` 的 `jobs:` 段，原样的字符串（`app` 不依赖实例，解析在这里）。
#[derive(Debug, Clone, Default)]
pub struct GuardText {
    pub timeout: Option<String>,
    pub max_tokens: Option<String>,
    pub overlap: Option<String>,
    pub disable_after: Option<u32>,
    pub network: Option<String>,
    pub limit_timeout: Option<String>,
    pub limit_max_tokens: Option<String>,
    pub daily_tokens: Option<String>,
    pub max_concurrent: Option<u32>,
}

impl GuardConfig {
    /// 读不懂的项用缺省，原因放进返回的列表（写进 host.log，配置写错不该让实例起不来）。
    pub fn parse(t: &GuardText) -> (GuardConfig, Vec<String>) {
        let mut c = GuardConfig::default();
        let mut problems = vec![];
        let mut take = |name: &str, v: &Option<String>, parse: fn(&str) -> Option<u64>| {
            let s = v.as_deref()?;
            let n = parse(s);
            if n.is_none() {
                problems.push(format!(
                    "jobs.{name}: {s:?} is not understood; the default is used"
                ));
            }
            n
        };
        if let Some(v) = take("defaults.timeout", &t.timeout, parse_duration) {
            c.defaults.timeout_ms = Some(v);
        }
        if let Some(v) = take("defaults.max_tokens", &t.max_tokens, parse_tokens) {
            c.defaults.max_tokens = Some(v);
        }
        if let Some(v) = take("limits.timeout", &t.limit_timeout, parse_duration) {
            c.max_timeout_ms = v;
        }
        if let Some(v) = take("limits.max_tokens", &t.limit_max_tokens, parse_tokens) {
            c.max_tokens = v;
        }
        match t.daily_tokens.as_deref().map(str::trim) {
            Some("0") => c.daily_tokens = 0,
            Some(s) => match parse_tokens(s) {
                Some(v) => c.daily_tokens = v,
                None => problems.push(format!(
                    "jobs.daily_tokens: {s:?} is not understood; the default is used"
                )),
            },
            None => {}
        }
        match t.overlap.as_deref().map(str::trim) {
            Some("skip") => c.defaults.overlap = Some(Overlap::Skip),
            Some("queue") => c.defaults.overlap = Some(Overlap::Queue),
            Some("replace") => c.defaults.overlap = Some(Overlap::Replace),
            Some(s) => problems.push(format!(
                "jobs.defaults.overlap: {s:?} is not skip, queue or replace; the default is used"
            )),
            None => {}
        }
        if let Some(n) = t.disable_after {
            c.defaults.disable_after = Some(n);
        }
        if let Some(s) = t.network.as_deref().map(str::trim) {
            match Network::parse(s) {
                Some(n) => c.defaults.network = Some(n),
                None => problems.push(format!(
                    "jobs.defaults.network: {s:?} is not deny_all, allowlist or inherit; the default is used"
                )),
            }
        }
        if let Some(n) = t.max_concurrent {
            c.max_concurrent = n.max(1);
        }
        (c, problems)
    }
}

/// `30m`、`2h`、`90s`、`1h30m`；纯数字按分钟。
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return Some(n * MIN);
    }
    let (mut total, mut num) = (0u64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            's' => 1_000,
            'm' => MIN,
            'h' => 60 * MIN,
            'd' => 24 * 60 * MIN,
            _ => return None,
        };
    }
    num.is_empty().then_some(total).filter(|t| *t > 0)
}

/// `2M`、`500k`、`1.5M`、纯数字。
pub fn parse_tokens(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1_000.0),
        'm' | 'M' => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    let v: f64 = num.trim().parse().ok()?;
    (v > 0.0).then_some((v * mul) as u64)
}

pub fn fmt_duration(ms: u64) -> String {
    let m = ms / MIN;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_yaml_section_is_parsed_and_bad_values_fall_back() {
        let (c, problems) = GuardConfig::parse(&GuardText {
            timeout: Some("45m".into()),
            max_tokens: Some("lots".into()),
            overlap: Some("queue".into()),
            disable_after: Some(5),
            network: Some("everywhere".into()),
            limit_timeout: Some("2h".into()),
            limit_max_tokens: None,
            daily_tokens: Some("0".into()),
            max_concurrent: Some(4),
        });
        assert_eq!(c.defaults.timeout_ms, Some(45 * MIN));
        assert_eq!(c.defaults.max_tokens, Some(2_000_000), "读不懂的用缺省");
        assert_eq!(c.defaults.overlap, Some(Overlap::Queue));
        assert_eq!(c.defaults.disable_after, Some(5));
        assert_eq!(c.max_timeout_ms, 120 * MIN);
        assert_eq!(c.daily_tokens, 0, "0 是不限");
        assert_eq!(c.max_concurrent, 4);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(c.defaults.network, Some(Network::DenyAll), "读不懂的用缺省");
    }

    #[test]
    fn durations_and_token_counts_parse() {
        assert_eq!(parse_duration("30m"), Some(30 * MIN));
        assert_eq!(parse_duration("1h30m"), Some(90 * MIN));
        assert_eq!(parse_duration("45"), Some(45 * MIN));
        assert_eq!(parse_duration("90s"), Some(90_000));
        assert_eq!(parse_duration("3x"), None);
        assert_eq!(parse_duration("1h3"), None);
        assert_eq!(parse_tokens("2M"), Some(2_000_000));
        assert_eq!(parse_tokens("1.5m"), Some(1_500_000));
        assert_eq!(parse_tokens("500k"), Some(500_000));
        assert_eq!(parse_tokens("abc"), None);
    }

    /// 仓库里的作业放不宽这台机器的天花板；没写的用缺省。
    #[test]
    fn a_jobs_own_guard_is_capped_by_the_machines_limits() {
        let c = GuardConfig::default();
        let (g, capped) = c.effective(&JobGuard {
            timeout_ms: Some(48 * 60 * MIN),
            max_tokens: Some(500_000),
            overlap: Some(Overlap::Queue),
            disable_after: None,
            network: Some(Network::Inherit),
        });
        assert_eq!(g.timeout_ms, Some(240 * MIN));
        assert_eq!(g.max_tokens, Some(500_000));
        assert_eq!(g.overlap, Some(Overlap::Queue));
        assert_eq!(g.disable_after, Some(3));
        assert_eq!(g.network, Some(Network::Inherit), "作业写了就照它");
        assert_eq!(capped, ["timeout"]);
        let (d, capped) = c.effective(&JobGuard::default());
        assert_eq!(d, c.defaults);
        assert!(capped.is_empty());
    }
}
