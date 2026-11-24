//! 界面文字的多语言（需求 L-11）。文字放在 `locales/<语言>/<模块>.yaml`，键是 YAML 路径
//! 拼起来的 `模块.名字`；代码里用 `t!("panel.collapse")`、`t!("flows.progress", done = 3)`
//! 取当前语言的文字，`{名字}` 换成参数。
//!
//! - 内置语言编进二进制（`build.rs` 扫 `locales/` 下的目录）；`$CONIX_HOME/locales/<语言>/`
//!   里的文件在启动时叠加上去，所以加一种语言、改个别说法都只要放一个资源文件。
//! - `zh` 是参照：每个键都必须有；别的语言缺了某个键就退回中文，再缺就显示键名。
//! - 没调用 `init` 时是中文（测试与快照都按中文写）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, RwLock};

include!(concat!(env!("OUT_DIR"), "/builtin.rs"));

/// 参照语言：每个键都有，其它语言缺键时退回到它。
pub const REFERENCE: &str = "zh";

struct Catalog {
    lang: String,
    texts: HashMap<String, String>,
    fallback: Arc<HashMap<String, String>>,
}

static REFERENCE_TEXTS: LazyLock<Arc<HashMap<String, String>>> = LazyLock::new(|| {
    Arc::new(builtin(REFERENCE).unwrap_or_else(|e| panic!("内置中文资源有错：{e}")))
});

static CURRENT: LazyLock<RwLock<Arc<Catalog>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(Catalog {
        lang: REFERENCE.into(),
        texts: HashMap::new(),
        fallback: REFERENCE_TEXTS.clone(),
    }))
});

/// 编进二进制的语言。
pub fn builtin_languages() -> Vec<&'static str> {
    BUILTIN.iter().map(|(l, _)| *l).collect()
}

/// 当前语言。
pub fn lang() -> String {
    CURRENT.read().unwrap().lang.clone()
}

/// 某个内置语言的全部文字（键已拼好）。测试与工具用。
pub fn builtin(lang: &str) -> Result<HashMap<String, String>, String> {
    let (_, files) = BUILTIN
        .iter()
        .find(|(l, _)| *l == lang)
        .ok_or_else(|| format!("没有内置语言 {lang}"))?;
    let mut out = HashMap::new();
    for (file, text) in *files {
        parse_into(text, &mut out).map_err(|e| format!("{lang}/{file}：{e}"))?;
    }
    Ok(out)
}

/// 切换语言。`extra` 是用户的资源目录（`$CONIX_HOME/locales`），其中 `<lang>/*.yaml` 叠加在
/// 内置的之上；内置里没有、用户目录里有的语言也可以用。返回加载时遇到的问题（不致命）。
pub fn init(lang: &str, extra: Option<&Path>) -> Vec<String> {
    let mut problems = vec![];
    let mut texts = if lang == REFERENCE {
        HashMap::new()
    } else {
        builtin(lang).unwrap_or_default()
    };
    let mut found = BUILTIN.iter().any(|(l, _)| *l == lang);
    if let Some(dir) = extra.map(|d| d.join(lang))
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        files.sort();
        for f in files {
            found = true;
            match std::fs::read_to_string(&f) {
                Ok(text) => {
                    if let Err(e) = parse_into(&text, &mut texts) {
                        problems.push(format!("{}：{e}", f.display()));
                    }
                }
                Err(e) => problems.push(format!("{}：{e}", f.display())),
            }
        }
    }
    if !found {
        problems.push(format!("没有语言 {lang} 的资源，用中文"));
    }
    *CURRENT.write().unwrap() = Arc::new(Catalog {
        lang: if found { lang.into() } else { REFERENCE.into() },
        texts,
        fallback: REFERENCE_TEXTS.clone(),
    });
    problems
}

/// 当前语言里 `key` 的文字；缺了退回中文，再缺显示键名（一眼看得出漏了哪个）。
pub fn tr(key: &str) -> String {
    let c = CURRENT.read().unwrap().clone();
    c.texts
        .get(key)
        .or_else(|| c.fallback.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// 带参数：文字里的 `{名字}` 换成对应的值。
pub fn tr_args(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(&tr(key), args)
}

/// 把 `{名字}` 换成参数。没给的占位原样留着。
pub fn fill(text: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = text.to_string();
    for (name, value) in args {
        let pat = format!("{{{name}}}");
        if out.contains(&pat) {
            out = out.replace(&pat, &value.to_string());
        }
    }
    out
}

/// 取文字：`t!("panel.collapse")`、`t!("flows.progress", done = d, total = n)`。
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::tr($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::tr_args($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// YAML 的嵌套映射拼成 `a.b.c` 键；叶子必须是字符串。
fn parse_into(text: &str, out: &mut HashMap<String, String>) -> Result<(), String> {
    let v: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    flatten("", &v, out)
}

fn flatten(
    prefix: &str,
    v: &serde_yaml::Value,
    out: &mut HashMap<String, String>,
) -> Result<(), String> {
    match v {
        serde_yaml::Value::Mapping(m) => {
            for (k, v) in m {
                let k = k
                    .as_str()
                    .ok_or_else(|| format!("{prefix} 下有不是字符串的键"))?;
                let key = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out)?;
            }
            Ok(())
        }
        serde_yaml::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
            Ok(())
        }
        serde_yaml::Value::Null if prefix.is_empty() => Ok(()),
        _ => Err(format!("{prefix} 的值必须是字符串")),
    }
}

/// 文字里的占位名，按出现顺序去重。
pub fn placeholders(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        rest = &rest[i + 1..];
        if let Some(j) = rest.find('}') {
            let name = &rest[..j];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let name = name.to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            rest = &rest[j + 1..];
        }
    }
    out
}

/// 从语言名或 `LANG` 这类环境变量的值里认出语言：`zh_CN.UTF-8` → `zh`。
pub fn normalize(tag: &str) -> String {
    tag.split(['_', '-', '.', '@'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_language_has_exactly_the_reference_keys_and_placeholders() {
        let zh = builtin(REFERENCE).unwrap();
        for lang in builtin_languages() {
            let other = builtin(lang).unwrap();
            let mut missing: Vec<_> = zh.keys().filter(|k| !other.contains_key(*k)).collect();
            let mut extra: Vec<_> = other.keys().filter(|k| !zh.contains_key(*k)).collect();
            missing.sort();
            extra.sort();
            assert!(missing.is_empty(), "{lang} 缺这些键：{missing:?}");
            assert!(extra.is_empty(), "{lang} 多了中文里没有的键：{extra:?}");
            for (k, v) in &zh {
                let mut a = placeholders(v);
                let mut b = placeholders(&other[k]);
                a.sort();
                b.sort();
                assert_eq!(a, b, "{lang} 的 {k} 占位和中文不一致");
            }
        }
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        let s = fill(
            "站 {done}/{total} {cur}",
            &[("done", &3), ("total", &7), ("cur", &"实现")],
        );
        assert_eq!(s, "站 3/7 实现");
    }

    #[test]
    fn language_tags_from_env() {
        assert_eq!(normalize("zh_CN.UTF-8"), "zh");
        assert_eq!(normalize("en-US"), "en");
        assert_eq!(normalize("C"), "c");
    }
}
