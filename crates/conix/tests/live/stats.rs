//! LIVE 的统计（testing.md §8.2）：每个运行（连同子运行）每一站的模型、用时、token，按模型汇总，再加各家的额度台账。
//! 从项目里的运行记录（`flow-runs/<运行>.json`）算：worktree 的运行合回项目之后，它过程目录里的报告就跟着 worktree 没了，
//! 运行记录还在。用例结束、拍快照时打一张表，并追加一行进 `target/tmp/live-stats.jsonl`，看得到历次的走势。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::Live;

/// 运行记录里 `${settings.<路径>}` 的值：钉在运行里的设置。
fn resolve(v: &Value, settings: &Value) -> Option<String> {
    let s = v.as_str()?;
    match s
        .strip_prefix("${settings.")
        .and_then(|r| r.strip_suffix('}'))
    {
        Some(path) => path
            .split('.')
            .try_fold(settings, |v, k| v.get(k))
            .and_then(Value::as_str)
            .map(String::from),
        None => Some(s.to_string()),
    }
}

fn usage_of(u: &Value) -> [u64; 4] {
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    [
        n("input_tokens"),
        n("output_tokens"),
        n("cache_read_input_tokens"),
        n("cache_creation_input_tokens"),
    ]
}

fn add(a: &mut [u64; 4], b: [u64; 4]) {
    for i in 0..4 {
        a[i] += b[i];
    }
}

/// 一个运行与它的子运行（深度优先），按 id。
fn tree(lv: &Live, run: &str) -> Vec<(String, Value)> {
    let mut out = vec![];
    let mut todo = vec![run.to_string()];
    while let Some(id) = todo.pop() {
        let Some(rec) = lv.try_state(&format!("flow-runs/{id}.json")) else {
            continue;
        };
        for st in rec["stations"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, s)| s)
        {
            for t in st["tasks"].as_array().into_iter().flatten() {
                if let Some(c) = t["child"].as_str() {
                    todo.push(c.to_string());
                }
            }
        }
        out.push((id, rec));
    }
    out
}

/// 这几个运行的每一站：`{run, station, model, status, rounds, ms, tokens: [in, out, cache_read, cache_write]}`。
pub fn rows(lv: &Live, runs: &[String]) -> Vec<Value> {
    let mut out = vec![];
    for run in runs {
        for (id, rec) in tree(lv, run) {
            let settings = &rec["parts"]["settings"];
            let main = resolve(&rec["def"]["main"]["model"], settings);
            for def in rec["def"]["stations"].as_array().into_iter().flatten() {
                let sid = def["id"].as_str().unwrap_or_default();
                let st = &rec["stations"][sid];
                if st.is_null() {
                    continue;
                }
                let ex = &def["executor"];
                let forks = def["tasks"]
                    .as_array()
                    .is_some_and(|t| t.iter().any(|t| t["executor"]["kind"] == "subflow"));
                let model = match ex["kind"].as_str() {
                    _ if forks => Some("（子运行）".into()),
                    Some("agent") => resolve(&ex["model"], settings).or(main.clone()),
                    Some("session") => main.clone(),
                    _ => None,
                }
                .unwrap_or_else(|| "—".into());
                let mut tokens = usage_of(&st["spent"]);
                for t in st["tasks"].as_array().into_iter().flatten() {
                    add(&mut tokens, usage_of(&t["usage"]));
                }
                let ms = match (st["entered_at"].as_u64(), st["left_at"].as_u64()) {
                    (Some(a), Some(b)) => b.saturating_sub(a),
                    _ => 0,
                };
                out.push(json!({
                    "run": id, "station": sid, "model": model, "status": st["status"],
                    "rounds": st["rounds_total"].as_u64().unwrap_or(st["round"].as_u64().unwrap_or(0)),
                    "ms": ms, "tokens": tokens,
                }));
            }
            // 主会话里不属于哪一站的那些轮（启动时、站与站之间）。
            let other = usage_of(&rec["usage_other"]);
            if other.iter().any(|n| *n > 0) {
                out.push(json!({"run": id, "station": "（站外）", "model": main.clone().unwrap_or_else(|| "—".into()),
                                "status": "", "rounds": 0, "ms": 0, "tokens": other}));
            }
        }
    }
    out
}

/// 各家额度台账（`<CONIX_HOME>/quota/<provider>.json`）原样。
pub fn quota(lv: &Live) -> Value {
    let dir = lv.home.path().join("quota");
    let mut out = serde_json::Map::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e
            .file_name()
            .to_string_lossy()
            .trim_end_matches(".json")
            .to_string();
        if let Ok(t) = std::fs::read_to_string(e.path())
            && let Ok(v) = serde_json::from_str(&t)
        {
            out.insert(name, v);
        }
    }
    Value::Object(out)
}

fn k(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// 打一张表（stderr），回 `{rows, by_model, quota}`。
pub fn report(lv: &Live, what: &str, runs: &[String]) -> Value {
    let rows = rows(lv, runs);
    let mut by_model: BTreeMap<String, [u64; 4]> = BTreeMap::new();
    let mut text = format!(
        "==== 统计：{what}\n{:<28} {:<20} {:<22} {:>3} {:>8} {:>9} {:>8} {:>9}\n",
        "运行", "站", "模型", "轮", "用时", "输入", "输出", "缓存读"
    );
    for r in &rows {
        let t: [u64; 4] = serde_json::from_value(r["tokens"].clone()).unwrap_or_default();
        add(
            by_model
                .entry(r["model"].as_str().unwrap_or("—").to_string())
                .or_default(),
            t,
        );
        text.push_str(&format!(
            "{:<28} {:<20} {:<22} {:>3} {:>7}s {:>9} {:>8} {:>9}\n",
            r["run"].as_str().unwrap_or_default(),
            r["station"].as_str().unwrap_or_default(),
            r["model"].as_str().unwrap_or_default(),
            r["rounds"],
            r["ms"].as_u64().unwrap_or(0) / 1000,
            k(t[0]),
            k(t[1]),
            k(t[2]),
        ));
    }
    text.push_str("---- 按模型\n");
    by_model.retain(|_, t| t.iter().any(|n| *n > 0));
    for (m, t) in &by_model {
        text.push_str(&format!(
            "{m:<22} 输入 {} · 输出 {} · 缓存读 {} · 缓存写 {}\n",
            k(t[0]),
            k(t[1]),
            k(t[2]),
            k(t[3])
        ));
    }
    let quota = quota(lv);
    text.push_str(&format!("---- 额度台账\n{quota}\n"));
    eprintln!("{text}");
    json!({"rows": rows, "by_model": by_model, "quota": quota})
}

/// 用例结束时：打表并追加一行进 `live-stats.jsonl`。
pub fn finish(lv: &Live, case: &str, runs: &[String], elapsed_s: u64) -> Value {
    let v = report(lv, case, runs);
    let line = json!({"case": case, "at_ms": super::now_ms(), "elapsed_s": elapsed_s,
                      "by_model": v["by_model"], "quota": v["quota"]});
    let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("live-stats.jsonl");
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = writeln!(f, "{line}");
    }
    v
}
