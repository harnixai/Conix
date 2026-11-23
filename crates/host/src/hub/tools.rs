//! 实例给每个会话的宿主工具（architecture §8.5）：会话与作业的查看、修改。经
//! `daemon.setHostTools` 挂在连接上，所以每个会话都有；能动什么由调用它的会话决定
//! （[`Scope`]），实例执行前查，不靠提示词。
//! 工具的说明与参数是给模型看的，不是界面文字，所以用英文写、不进语言资源。

use port::{Job, JobGuard, JobId, JobKind, Origin, Overlap};

use crate::jobs::guard::{parse_duration, parse_tokens};
use serde_json::{Value, json};

pub const SESSION_LIST: &str = "session_list";
pub const SESSION_SHOW: &str = "session_show";
pub const SESSION_RENAME: &str = "session_rename";
pub const SESSION_CLOSE: &str = "session_close";
pub const SESSION_DELETE: &str = "session_delete";
pub const JOB_LIST: &str = "job_list";
pub const JOB_SHOW: &str = "job_show";
pub const JOB_SAVE: &str = "job_save";
pub const JOB_DELETE: &str = "job_delete";
pub const JOB_ENABLE: &str = "job_enable";
pub const JOB_RUN: &str = "job_run";
pub const JOB_VALIDATE: &str = "job_validate";
pub const JOB_HISTORY: &str = "job_history";
pub const JOB_STOP: &str = "job_stop";

/// 只读的几个：建会话时给它们 allow 规则，查看不用每次问人。
pub const READ_ONLY: [&str; 6] = [
    SESSION_LIST,
    SESSION_SHOW,
    JOB_LIST,
    JOB_SHOW,
    JOB_VALIDATE,
    JOB_HISTORY,
];

/// 一次调用能动什么，由发起调用的会话（子 Agent 的调用算它的根会话）决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// 用户会话（人新建的、分叉的）：一切。
    Full,
    /// 作业触发出来的会话：只动触发它的那个作业，会话只动自己。
    Job(String),
    /// 流程运行里的会话：作业一个都不动，会话只动自己。
    Run(String),
    /// 不是这个实例管的会话：什么都不给。
    None,
}

pub fn scope_of(origin: Option<&Origin>) -> Scope {
    match origin {
        Some(Origin::User | Origin::Fork { .. }) => Scope::Full,
        Some(Origin::Job { job, .. }) => Scope::Job(job.0.clone()),
        Some(Origin::FlowStation { run, .. }) => Scope::Run(run.0.clone()),
        // 侧链会话不会是发起者（子 Agent 的调用用根会话回答）；真碰上了按最窄的算。
        Some(Origin::SubAgent { .. }) | None => Scope::None,
    }
}

const TARGET: &str = "\"#3\" (the number shown in the list right now) or the id. Numbers follow the current order and can shift when the list changes; ids never do.";

fn target(desc: &str) -> Value {
    json!({"type": "string", "description": format!("{desc}: {TARGET}")})
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let read_only = READ_ONLY.contains(&name);
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        },
        "read_only": read_only,
        "destructive": matches!(name, SESSION_CLOSE | SESSION_DELETE | JOB_DELETE | JOB_STOP),
        "subagents": true
    })
}

pub fn declarations() -> Value {
    let job_fields = json!({
        "name": {"type": "string"},
        "cron": {"type": "string", "description": "5-field cron in the local timezone (minute hour day-of-month month day-of-week), e.g. \"0 9 * * 1-5\"."},
        "kind": {"type": "string", "enum": ["simple", "flow"], "description": "simple: each firing starts an unattended session that does `prompt`. flow: each firing starts a run of `flow` on `subject`. Default simple; a job keeps its kind unless this is given."},
        "prompt": {"type": "string", "description": "simple jobs: what the session started by each firing should do."},
        "skills": {"type": "array", "items": {"type": "string"}, "description": "simple jobs: skills that session should use."},
        "model": {"type": "string", "description": "simple jobs: the model for that session."},
        "flow": {"type": "string", "description": "flow jobs: the flow, \"#n\" as in the flows list or its name."},
        "subject": {"type": "string", "description": "flow jobs: the subject of each run."},
        "inputs": {"type": "object", "description": "flow jobs: the run's inputs; they must match the flow's inputs schema (flow_show)."},
        "enabled": {"type": "boolean"},
        "catch_up": {"type": "boolean", "description": "Run once at startup if a firing was missed while the instance was down."},
        "timeout": {"type": ["string", "null"], "description": "Guardrail: stop a firing running longer than this, e.g. \"30m\", \"2h\". null = the machine's default. Capped by the machine's limit."},
        "max_tokens": {"type": ["string", "integer", "null"], "description": "Guardrail: stop a firing using more tokens than this (input + output, sub-agents included), e.g. \"2M\", \"500k\". null = the machine's default. Capped by the machine's limit."},
        "overlap": {"type": ["string", "null"], "enum": ["skip", "queue", "replace", null], "description": "Guardrail: when a firing is due while the previous one still runs: skip it, queue one, or replace (stop the previous). null = the machine's default."},
        "disable_after": {"type": ["integer", "null"], "description": "Guardrail: disable the job on this machine after this many failures in a row; 0 = never. null = the machine's default."}
    });
    let mut save_props = job_fields.as_object().cloned().unwrap_or_default();
    save_props.insert(
        "target".into(),
        target("The job to change; omit to create a new one"),
    );
    json!([
        tool(
            SESSION_LIST,
            "List the sessions of this Codix instance's project: the user's sessions, numbered as in the sessions list (#1 is the newest). With all=true, also the sessions started by scheduled jobs and flow runs, and the closed ones (not numbered).",
            json!({"all": {"type": "boolean", "default": false}}),
            &[]
        ),
        tool(
            SESSION_SHOW,
            "Show one session: title, state, origin, model, usage, goal, todos and background tasks.",
            json!({"target": target("The session")}),
            &["target"]
        ),
        tool(
            SESSION_RENAME,
            "Rename a session; this is the name shown in the sessions list. Omit target to rename the session you are in.",
            json!({"target": target("The session; omit for this one"), "title": {"type": "string"}}),
            &["title"]
        ),
        tool(
            SESSION_CLOSE,
            "Close a session: it leaves the sessions list (its transcript is kept). Only when the user asks for it.",
            json!({"target": target("The session")}),
            &["target"]
        ),
        tool(
            SESSION_DELETE,
            "Delete a session for good: its transcript, memory and sub-agent sessions. A session open in the sessions list has to be closed first; closed ones are listed with all=true. Only when the user asks for it.",
            json!({"target": target("The session (a closed one: its id)")}),
            &["target"]
        ),
        tool(
            JOB_LIST,
            "List the scheduled jobs of this project, numbered as in the jobs list: kind, name, cron, enabled (or disabled after failures), next firing, the firing running now, last outcome.",
            json!({}),
            &[]
        ),
        tool(
            JOB_SHOW,
            "Show one scheduled job: its whole definition, its guardrails as they take effect on this machine, and its 10 most recent firings.",
            json!({"target": target("The job")}),
            &["target"]
        ),
        tool(
            JOB_SAVE,
            "Create or change a scheduled job. A simple job's firing starts a new unattended session in this project and submits `prompt` as its first turn; a flow job's firing starts a run of `flow` on `subject`. Without target this creates a job (name, cron, and prompt or flow + subject required); with target only the given fields change. Check it with job_validate first and confirm the schedule with the user before saving.",
            Value::Object(save_props.clone()),
            &[]
        ),
        tool(
            JOB_VALIDATE,
            "Check a job definition without saving it: the problems that would stop it from being saved, its next 5 firings (local time) and its guardrails as they would take effect. Takes the same fields as job_save; with target, the given fields are applied to that job.",
            Value::Object(save_props),
            &[]
        ),
        tool(
            JOB_HISTORY,
            "List a job's firings, newest first: when, outcome (and why it was stopped or skipped), time, tokens, and the session or run it started.",
            json!({
                "target": target("The job"),
                "before_ms": {"type": "integer", "description": "Only firings before this time (ms since the epoch), to page back."},
                "limit": {"type": "integer", "default": 20},
                "outcome": {"type": "string", "enum": ["running", "done", "failed", "stopped", "skipped"]}
            }),
            &["target"]
        ),
        tool(
            JOB_STOP,
            "Stop a job's firing that is running now: a simple job's session is interrupted, a flow job's run is cancelled. Only when the user asks for it.",
            json!({
                "target": target("The job"),
                "fired_ms": {"type": "integer", "description": "Which firing (its time); omit for the one running now."}
            }),
            &["target"]
        ),
        tool(
            JOB_DELETE,
            "Delete a scheduled job and its firing history. Only when the user asks for it.",
            json!({"target": target("The job")}),
            &["target"]
        ),
        tool(
            JOB_ENABLE,
            "Enable or disable a scheduled job.",
            json!({"target": target("The job"), "enabled": {"type": "boolean"}}),
            &["target", "enabled"]
        ),
        tool(
            JOB_RUN,
            "Fire a scheduled job once now, outside its schedule. For this firing only, a flow job can take another subject and a simple job's prompt can get an extra line; the job itself does not change.",
            json!({
                "target": target("The job"),
                "subject": {"type": "string", "description": "flow jobs: the subject for this run only."},
                "prompt_append": {"type": "string", "description": "simple jobs: appended to the prompt for this firing only."}
            }),
            &["target"]
        ),
    ])
}

pub fn str_of(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(Value::as_str).map(str::to_string)
}

/// `job_save` / `job_validate` 的参数 → 作业；未给的字段沿用已有作业的。流程作业的 `flow` 在这里
/// 还是人给的写法（`#n` 或名字），存之前由实例认成流程名。
pub fn job_from_input(input: &Value, existing: Option<Job>) -> Result<Job, String> {
    let creating = existing.is_none();
    let mut job = existing.unwrap_or(Job {
        id: JobId(String::new()),
        name: String::new(),
        cron: String::new(),
        kind: JobKind::Simple {
            prompt: String::new(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        created_ms: 0,
        guard: Default::default(),
        auto_disabled: None,
        effective_guard: None,
        next_fire_ms: None,
        last: None,
        running_ms: None,
        today_tokens: 0,
    });
    let to_flow = match str_of(input, "kind").as_deref() {
        Some("flow") => true,
        Some("simple") => false,
        Some(k) => return Err(format!("kind must be simple or flow, not {k}")),
        None => {
            matches!(job.kind, JobKind::Flow { .. })
                || (creating && input.get("flow").is_some() && input.get("prompt").is_none())
        }
    };
    if creating {
        let need: &[&str] = if to_flow {
            &["name", "cron", "flow", "subject"]
        } else {
            &["name", "cron", "prompt"]
        };
        for k in need {
            if str_of(input, k).is_none_or(|v| v.trim().is_empty()) {
                return Err(format!(
                    "{k} is required to create a {} job",
                    if to_flow { "flow" } else { "simple" }
                ));
            }
        }
    }
    if let Some(n) = str_of(input, "name") {
        job.name = n;
    }
    if let Some(c) = str_of(input, "cron") {
        job.cron = c;
    }
    if let Some(e) = input.get("enabled").and_then(Value::as_bool) {
        job.enabled = e;
    }
    if let Some(c) = input.get("catch_up").and_then(Value::as_bool) {
        job.catch_up = c;
    }
    job.kind = match (to_flow, &job.kind) {
        (
            true,
            JobKind::Flow {
                flow,
                subject,
                inputs,
            },
        ) => JobKind::Flow {
            flow: str_of(input, "flow").unwrap_or_else(|| flow.clone()),
            subject: str_of(input, "subject").unwrap_or_else(|| subject.clone()),
            inputs: input
                .get("inputs")
                .cloned()
                .unwrap_or_else(|| inputs.clone()),
        },
        (true, JobKind::Simple { .. }) => JobKind::Flow {
            flow: str_of(input, "flow").ok_or("flow is required to make this a flow job")?,
            subject: str_of(input, "subject")
                .ok_or("subject is required to make this a flow job")?,
            inputs: input.get("inputs").cloned().unwrap_or(Value::Null),
        },
        (
            false,
            JobKind::Simple {
                prompt,
                skills,
                project,
                scene,
                model,
            },
        ) => JobKind::Simple {
            prompt: str_of(input, "prompt").unwrap_or_else(|| prompt.clone()),
            skills: strings(input, "skills").unwrap_or_else(|| skills.clone()),
            project: project.clone(),
            scene: scene.clone(),
            model: str_of(input, "model").or_else(|| model.clone()),
        },
        (false, JobKind::Flow { .. }) => JobKind::Simple {
            prompt: str_of(input, "prompt")
                .ok_or("prompt is required to make this a simple job")?,
            skills: strings(input, "skills").unwrap_or_default(),
            project: None,
            scene: None,
            model: str_of(input, "model"),
        },
    };
    job.guard = guard_from_input(input, job.guard)?;
    if job.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    Ok(job)
}

fn strings(input: &Value, k: &str) -> Option<Vec<String>> {
    input.get(k).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    })
}

/// 护栏的几项：给了就改，给 `null` 就回到全局缺省，没给就不动。
fn guard_from_input(input: &Value, mut g: JobGuard) -> Result<JobGuard, String> {
    if let Some(v) = input.get("timeout") {
        g.timeout_ms = match v {
            Value::Null => None,
            Value::String(s) => {
                Some(parse_duration(s).ok_or(format!("timeout {s:?}: write it like 30m or 2h"))?)
            }
            Value::Number(n) => Some(n.as_u64().ok_or("timeout must be positive")? * 60_000),
            _ => return Err("timeout must be like 30m".into()),
        };
    }
    if let Some(v) = input.get("max_tokens") {
        g.max_tokens = match v {
            Value::Null => None,
            Value::String(s) => {
                Some(parse_tokens(s).ok_or(format!("max_tokens {s:?}: write it like 2M or 500k"))?)
            }
            Value::Number(n) => Some(
                n.as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("max_tokens must be positive")?,
            ),
            _ => return Err("max_tokens must be like 2M".into()),
        };
    }
    if let Some(v) = input.get("overlap") {
        g.overlap = match v.as_str() {
            None if v.is_null() => None,
            Some("skip") => Some(Overlap::Skip),
            Some("queue") => Some(Overlap::Queue),
            Some("replace") => Some(Overlap::Replace),
            _ => return Err("overlap must be skip, queue or replace".into()),
        };
    }
    if let Some(v) = input.get("disable_after") {
        g.disable_after = match v {
            Value::Null => None,
            v => Some(
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("disable_after must be a whole number")?,
            ),
        };
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_input_creates_with_the_required_fields_and_keeps_the_rest_on_change() {
        assert!(job_from_input(&json!({"name":"x","cron":"0 2 * * *"}), None).is_err());
        let j = job_from_input(
            &json!({"name":"nightly","cron":"0 2 * * *","prompt":"run tests","skills":["ci"]}),
            None,
        )
        .unwrap();
        assert!(j.enabled && j.id.0.is_empty());
        let j2 = job_from_input(&json!({"enabled":false}), Some(j)).unwrap();
        assert!(!j2.enabled);
        let JobKind::Simple { prompt, skills, .. } = &j2.kind else {
            panic!()
        };
        assert_eq!((prompt.as_str(), skills.len()), ("run tests", 1));
    }

    #[test]
    fn every_tool_is_declared_once_and_reaches_sub_agents() {
        let d = declarations();
        let names: Vec<&str> = d
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        let mut uniq = names.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(names.len(), 14);
        assert_eq!(uniq.len(), names.len());
        assert!(d.as_array().unwrap().iter().all(|t| t["subagents"] == true));
        for r in READ_ONLY {
            assert!(names.contains(&r));
        }
    }

    #[test]
    fn save_input_makes_flow_jobs_switches_kinds_and_sets_guardrails() {
        assert!(
            job_from_input(
                &json!({"name":"x","cron":"0 2 * * *","kind":"flow","flow":"#1"}),
                None
            )
            .is_err()
        );
        let f = job_from_input(
            &json!({"name":"weekly","cron":"0 9 * * 1","flow":"#1","subject":"release","inputs":{"v":1},
                    "timeout":"45m","max_tokens":"1.5M","overlap":"queue","disable_after":5}),
            None,
        )
        .unwrap();
        let JobKind::Flow {
            flow,
            subject,
            inputs,
        } = &f.kind
        else {
            panic!("{:?}", f.kind)
        };
        assert_eq!(
            (flow.as_str(), subject.as_str(), inputs["v"].as_i64()),
            ("#1", "release", Some(1))
        );
        assert_eq!(f.guard.timeout_ms, Some(45 * 60_000));
        assert_eq!(f.guard.max_tokens, Some(1_500_000));
        assert_eq!(f.guard.overlap, Some(Overlap::Queue));
        assert_eq!(f.guard.disable_after, Some(5));
        // 只改主题：种类与别的都留着；null 让一项回到全局缺省。
        let f2 =
            job_from_input(&json!({"subject":"hotfix","timeout":null}), Some(f.clone())).unwrap();
        assert!(
            matches!(&f2.kind, JobKind::Flow { subject, flow, .. } if subject == "hotfix" && flow == "#1")
        );
        assert_eq!(f2.guard.timeout_ms, None);
        assert_eq!(f2.guard.max_tokens, Some(1_500_000));
        // 换成简单作业要给提示词。
        assert!(job_from_input(&json!({"kind":"simple"}), Some(f.clone())).is_err());
        let s = job_from_input(&json!({"kind":"simple","prompt":"check logs"}), Some(f)).unwrap();
        assert!(matches!(&s.kind, JobKind::Simple { prompt, .. } if prompt == "check logs"));
        assert!(job_from_input(&json!({"timeout":"soon"}), Some(s.clone())).is_err());
        assert!(job_from_input(&json!({"overlap":"wait"}), Some(s)).is_err());
    }

    #[test]
    fn scope_follows_the_calling_session_origin() {
        assert_eq!(scope_of(Some(&Origin::User)), Scope::Full);
        assert_eq!(
            scope_of(Some(&Origin::Job {
                job: JobId("j1".into()),
                name: "n".into(),
                fired_ms: 0
            })),
            Scope::Job("j1".into())
        );
        assert_eq!(scope_of(None), Scope::None);
    }
}
