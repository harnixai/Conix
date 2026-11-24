use std::collections::BTreeMap;

use port::{Command, Event, Millis};
use serde_yaml::{Mapping, Value};

use crate::MockError;

/// 一个场景：种子 + 自动发生的时间线 + 命令到反应的规则 + 可复用的片段。
#[derive(Debug, Clone, Default)]
pub struct Scenario {
    pub name: String,
    pub seed: String,
    pub timeline: Vec<Timed>,
    pub rules: Vec<Rule>,
    pub scripts: BTreeMap<String, Vec<Step>>,
}

/// 时间线上的一项：场景开始后 `at` 毫秒发生。
#[derive(Debug, Clone)]
pub struct Timed {
    pub at: Millis,
    pub action: StepAction,
}

/// 片段或规则反应里的一步：上一步之后 `after` 毫秒发生。
#[derive(Debug, Clone)]
pub struct Step {
    pub after: Millis,
    pub action: StepAction,
}

#[derive(Debug, Clone)]
pub enum StepAction {
    Event(Box<Event>),
    Play(String),
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub on: Matcher,
    /// 为空时用缺省反应的结果。
    pub reply: Option<RuleReply>,
    pub then: Vec<Step>,
    /// 只匹配一次：同一条命令第二次到来时落到下一条规则或缺省反应。
    pub once: bool,
}

#[derive(Debug, Clone)]
pub enum RuleReply {
    Ok,
    Error(String),
}

/// `on: submit` 匹配任何 `submit`；`on: {answer: {prompt: p2}}` 还要求字段是命令的子集。
#[derive(Debug, Clone)]
pub struct Matcher {
    pub command: String,
    pub fields: serde_json::Value,
}

impl Matcher {
    pub fn matches(&self, cmd: &Command) -> bool {
        if cmd.name() != self.command {
            return false;
        }
        let Ok(serde_json::Value::Object(mut whole)) = serde_json::to_value(cmd) else {
            return self.fields.is_null();
        };
        let body = whole.remove(self.command.as_str()).unwrap_or_default();
        subset(&self.fields, &body)
    }
}

fn subset(want: &serde_json::Value, have: &serde_json::Value) -> bool {
    use serde_json::Value as J;
    match (want, have) {
        (J::Null, _) => true,
        (J::Object(w), J::Object(h)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| subset(v, hv))),
        (w, h) => w == h,
    }
}

impl Scenario {
    pub fn parse(name: &str, text: &str) -> Result<Scenario, MockError> {
        let err = |e: String| MockError::Parse(name.to_string(), e);
        let root: Mapping = serde_yaml::from_str(text).map_err(|e| err(e.to_string()))?;
        let get = |k: &str| root.get(Value::from(k));

        let mut sc = Scenario {
            name: get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string(),
            seed: get("seed")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string(),
            ..Scenario::default()
        };
        if let Some(Value::Sequence(items)) = get("timeline") {
            for item in items {
                let (at, action) = step(item, "at").map_err(err)?;
                sc.timeline.push(Timed { at, action });
            }
        }
        if let Some(Value::Mapping(scripts)) = get("scripts") {
            for (k, v) in scripts {
                let key = k.as_str().ok_or_else(|| err("脚本名必须是字符串".into()))?;
                sc.scripts.insert(key.to_string(), steps(v).map_err(err)?);
            }
        }
        if let Some(Value::Sequence(rules)) = get("rules") {
            for r in rules {
                sc.rules.push(rule(r).map_err(err)?);
            }
        }
        Ok(sc)
    }

    /// 只有种子、没有时间线的场景（`conix --mock <种子名>`）。
    pub fn seed_only(seed: &str) -> Scenario {
        Scenario {
            name: seed.to_string(),
            seed: seed.to_string(),
            ..Scenario::default()
        }
    }
}

fn steps(v: &Value) -> Result<Vec<Step>, String> {
    let Value::Sequence(items) = v else {
        return Err("片段必须是列表".into());
    };
    items
        .iter()
        .map(|i| step(i, "after").map(|(after, action)| Step { after, action }))
        .collect()
}

/// `{after: 30, text_delta: {...}}` 或 `{after: 0, play: name}`。
fn step(v: &Value, time_key: &str) -> Result<(Millis, StepAction), String> {
    let Value::Mapping(m) = v else {
        return Err(format!("一步必须是映射：{v:?}"));
    };
    let mut m = m.clone();
    let time = m
        .remove(Value::from(time_key))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    if let Some(name) = m.remove(Value::from("play")) {
        let name = name.as_str().ok_or("play 的值必须是片段名")?;
        return Ok((time, StepAction::Play(name.to_string())));
    }
    if m.len() != 1 {
        return Err(format!("一步里只能有一个事件：{m:?}"));
    }
    let event: Event =
        crate::from_yaml_value(Value::Mapping(m)).map_err(|e| format!("事件解析失败：{e}"))?;
    Ok((time, StepAction::Event(Box::new(event))))
}

fn rule(v: &Value) -> Result<Rule, String> {
    let Value::Mapping(m) = v else {
        return Err("规则必须是映射".into());
    };
    let on = match m.get(Value::from("on")) {
        Some(Value::String(s)) => Matcher {
            command: s.clone(),
            fields: serde_json::Value::Null,
        },
        Some(Value::Mapping(om)) if om.len() == 1 => {
            let (k, v) = om.iter().next().unwrap();
            Matcher {
                command: k.as_str().ok_or("命令名必须是字符串")?.to_string(),
                fields: serde_json::to_value(v).map_err(|e| e.to_string())?,
            }
        }
        _ => return Err("规则缺少 on，或 on 不是 `命令` / `{命令: 字段}`".into()),
    };
    let reply = match m.get(Value::from("reply")) {
        None => None,
        Some(Value::String(s)) if s == "ok" => Some(RuleReply::Ok),
        Some(Value::Mapping(rm)) => match rm.get(Value::from("error")).and_then(Value::as_str) {
            Some(e) => Some(RuleReply::Error(e.to_string())),
            None => return Err("reply 只支持 ok 或 {error: …}".into()),
        },
        Some(other) => return Err(format!("reply 不认识：{other:?}")),
    };
    let then = match m.get(Value::from("then")) {
        Some(v) => steps(v)?,
        None => vec![],
    };
    let once = m
        .get(Value::from("once"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Rule {
        on,
        reply,
        then,
        once,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name: m04
seed: default
timeline:
  - at: 100
    prompt_pending:
      session: s1
      prompt: {id: p1, kind: {permission: {tool: Bash, summary: cargo test}}}
  - {at: 200, play: hello}
rules:
  - on: {answer: {prompt: p1}}
    reply: ok
    then:
      - {after: 0, prompt_resolved: {session: s1, id: p1, resolution: answered}}
  - on: submit
    once: true
    then: [{after: 10, play: hello}]
scripts:
  hello:
    - {after: 0, turn_started: {session: s1}}
    - {after: 30, text_delta: {session: s1, text: "你好"}}
"#;

    #[test]
    fn parses_timeline_rules_and_scripts() {
        let sc = Scenario::parse("t", SAMPLE).unwrap();
        assert_eq!(sc.timeline.len(), 2);
        assert_eq!(sc.timeline[0].at, 100);
        assert!(matches!(sc.timeline[1].action, StepAction::Play(ref n) if n == "hello"));
        assert_eq!(sc.scripts["hello"].len(), 2);
        assert_eq!(sc.rules.len(), 2);
        assert!(sc.rules[1].once);
    }

    #[test]
    fn matcher_checks_name_and_field_subset() {
        let sc = Scenario::parse("t", SAMPLE).unwrap();
        let hit = Command::Answer {
            session: "s1".into(),
            prompt: "p1".into(),
            answer: port::Answer::Permit,
        };
        let miss = Command::Answer {
            session: "s1".into(),
            prompt: "p2".into(),
            answer: port::Answer::Permit,
        };
        assert!(sc.rules[0].on.matches(&hit));
        assert!(!sc.rules[0].on.matches(&miss));
        assert!(sc.rules[1].on.matches(&Command::Submit {
            session: "x".into(),
            text: "t".into(),
            mode: port::SubmitMode::Turn,
        }));
    }

    #[test]
    fn bad_event_names_the_problem() {
        let e = Scenario::parse("t", "timeline: [{at: 0, no_such_event: {}}]").unwrap_err();
        assert!(e.to_string().contains("事件解析失败"), "{e}");
    }
}
