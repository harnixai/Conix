//! 人在 `/flow:<包> … --k v` 里打的值都是字：流程的输入 schema 要数、开关或列表的，在实例这一侧照 schema 读成那种值。
//! 只给人的那扇门（`Command::StartRun`）：模型的 `flow_start` 本来就交 JSON，宽松了反倒把它写错的类型悄悄放过去。
//! 读不成的原样留着，由流程的校验照常拒绝、说清哪个输入不合。

use harnix_flow::Packs;
use serde_json::{Map, Value};

/// `name` 这个名字可能起的流程：它自己，或一个包的每条流程。
fn candidates(packs: &dyn Packs, name: &str) -> Vec<harnix_flow::FlowDef> {
    match packs.flow(name) {
        Some((def, _)) => vec![def],
        None => harnix_flow::bundle::offered_stems(packs, name)
            .iter()
            .filter_map(|stem| {
                packs
                    .flow(&harnix_flow::bundle::flow_name(name, stem))
                    .map(|(d, _)| d)
            })
            .collect(),
    }
}

/// 人起运行时打的 `--title`：流程声明了 `title` 这个输入就是它；都没声明，它是运行的标题，换掉主题（界面不知道
/// 流程的输入，两样都交过来）。回主题与读过类型的输入。
pub(super) fn person_start(
    packs: &dyn Packs,
    name: &str,
    subject: String,
    inputs: Value,
) -> (String, Value) {
    let Value::Object(mut map) = inputs else {
        return (subject, inputs);
    };
    let declared = candidates(packs, name)
        .iter()
        .any(|d| d.inputs.contains_key("title"));
    let subject = match map.get("title") {
        Some(Value::String(t)) if !declared => {
            let t = t.clone();
            map.remove("title");
            t
        }
        _ => subject,
    };
    (subject, typed_inputs(packs, name, Value::Object(map)))
}

/// `inputs` 里的字照 `name`（一条流程，或一个包的每条流程）的输入 schema 读：几条流程说的类型不一样就不动。
pub(super) fn typed_inputs(packs: &dyn Packs, name: &str, inputs: Value) -> Value {
    let Value::Object(map) = inputs else {
        return inputs;
    };
    let defs = candidates(packs, name);
    let out: Map<String, Value> = map
        .into_iter()
        .map(|(k, v)| {
            let Value::String(s) = &v else { return (k, v) };
            let types: Vec<&str> = defs
                .iter()
                .filter_map(|d| d.inputs.get(&k))
                .map(|i| i.schema.get("type").and_then(Value::as_str).unwrap_or(""))
                .collect();
            let typed = match types.split_first() {
                Some((t, rest)) if rest.iter().all(|r| r == t) => read(t, s),
                _ => None,
            };
            (k, typed.unwrap_or(v))
        })
        .collect();
    Value::Object(out)
}

fn read(ty: &str, s: &str) -> Option<Value> {
    let s = s.trim();
    match ty {
        "integer" => s.parse::<i64>().ok().map(Value::from),
        "number" => s
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number)),
        "boolean" => match s {
            "true" | "yes" | "on" => Some(Value::Bool(true)),
            "false" | "no" | "off" => Some(Value::Bool(false)),
            _ => None,
        },
        // `--impact interface,security`：逗号分开的几个词。
        "array" => Some(Value::Array(
            s.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(|x| Value::String(x.into()))
                .collect(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harnix_flow::MemPacks;
    use serde_json::json;

    #[test]
    fn a_typed_word_is_read_as_the_schema_says_and_a_word_it_cannot_read_is_left() {
        let packs = MemPacks::new().flow(
            "name: r\ninputs:\n  days: {schema: {type: integer}}\n  impact: {schema: {type: array, items: {type: string}}}\n  dry: {schema: {type: boolean}}\n  risk: {schema: {enum: [low, high]}}\nstations: [{id: a, exit: []}]\n",
        );
        let got = typed_inputs(
            &packs,
            "r",
            json!({"days": "7", "impact": "interface, security", "dry": "true", "risk": "low", "title": "t"}),
        );
        assert_eq!(
            got,
            json!({"days": 7, "impact": ["interface", "security"], "dry": true, "risk": "low", "title": "t"})
        );
        assert_eq!(
            typed_inputs(&packs, "r", json!({"days": "a week"})),
            json!({"days": "a week"}),
            "the flow's check says what is wrong"
        );
        assert_eq!(
            typed_inputs(&packs, "nothing", json!({"days": "7"})),
            json!({"days": "7"})
        );
    }

    #[test]
    fn a_title_is_the_flows_input_when_it_declares_one_and_the_runs_subject_otherwise() {
        let packs = MemPacks::new()
            .flow("name: t\ninputs:\n  title: {schema: {type: string}}\nstations: [{id: a, exit: []}]\n")
            .flow("name: u\nstations: [{id: a, exit: []}]\n");
        let (subject, inputs) = person_start(
            &packs,
            "t",
            "加 wrap，按宽度折行".into(),
            json!({"title": "wrap"}),
        );
        assert_eq!(
            (subject.as_str(), inputs),
            ("加 wrap，按宽度折行", json!({"title": "wrap"}))
        );
        let (subject, inputs) =
            person_start(&packs, "u", "上周的提交".into(), json!({"title": "周报"}));
        assert_eq!((subject.as_str(), inputs), ("周报", json!({})));
    }
}
