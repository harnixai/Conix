//! 右栏三个列表的规范顺序。界面上的 `#n` 是对象在这个顺序里的位置（从 1 起），实例侧的工具
//! 解析 `#n` 也用这里的函数，所以两边说的 #3 是同一个东西。序号是临时的：列表一变就重排，
//! 长期指代一个对象用它的 ID。
//!
//! - 会话：只算用户会话（人新建的、分叉的），建会话的时刻新的在前；同一时刻的保持传入顺序。
//! - 流程：按名字（流程的唯一标识）升序。
//! - 作业：按创建时刻升序，同一时刻按 ID。ID 是随机的（定义进 git，序号会撞），所以不能按 ID 排；
//!   改名、改 cron 都不跳。

use crate::{FlowSummary, Job, Origin, SessionSummary};

/// 用户会话：会话标签里列出的那些。流程、作业、子 Agent 开出来的会话在别处。
pub fn is_user_session(s: &SessionSummary) -> bool {
    matches!(s.provenance.origin, Origin::User | Origin::Fork { .. })
}

pub fn sessions<'a>(all: impl IntoIterator<Item = &'a SessionSummary>) -> Vec<&'a SessionSummary> {
    let mut v: Vec<&SessionSummary> = all.into_iter().filter(|s| is_user_session(s)).collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.created_ms));
    v
}

pub fn flows<'a>(all: impl IntoIterator<Item = &'a FlowSummary>) -> Vec<&'a FlowSummary> {
    let mut v: Vec<&FlowSummary> = all.into_iter().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn jobs<'a>(all: impl IntoIterator<Item = &'a Job>) -> Vec<&'a Job> {
    let mut v: Vec<&Job> = all.into_iter().collect();
    v.sort_by(|a, b| (a.created_ms, &a.id.0).cmp(&(b.created_ms, &b.id.0)));
    v
}

/// `#n` 或 ID 指的是列表里的哪一项：`#3` / `3` 按位置，其余按 `id` 比。
pub fn resolve<'a, T>(ordered: &[&'a T], key: &str, id: impl Fn(&T) -> &str) -> Option<&'a T> {
    let k = key.trim();
    if let Ok(n) = k.trim_start_matches('#').parse::<usize>() {
        return n.checked_sub(1).and_then(|i| ordered.get(i)).copied();
    }
    ordered.iter().find(|x| id(x) == k).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FlowOrigin, InstanceName, JobId, JobKind, Provenance, SessionId, SessionState};

    fn sess(id: &str, created_ms: u64, origin: Origin) -> SessionSummary {
        SessionSummary {
            id: SessionId(id.into()),
            title: id.into(),
            state: SessionState::Idle,
            waiting: 0,
            usage: Default::default(),
            provenance: Provenance {
                instance: InstanceName::from("i"),
                host: "h".into(),
                project: None,
                scene: "code".into(),
                model: "m".into(),
                origin,
            },
            updated_ms: 0,
            created_ms,
            held_by: None,
            permission: Default::default(),
        }
    }

    #[test]
    fn sessions_are_user_ones_newest_first_ties_keep_input_order() {
        let all = [
            sess("a", 1, Origin::User),
            sess("b", 3, Origin::User),
            sess(
                "job",
                9,
                Origin::Job {
                    job: JobId("j".into()),
                    name: "j".into(),
                    fired_ms: 9,
                },
            ),
            sess("c", 1, Origin::User),
        ];
        let v = sessions(&all);
        let ids: Vec<&str> = v.iter().map(|s| s.id.0.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert_eq!(resolve(&v, "#2", |s| &s.id.0).unwrap().id.0, "a");
        assert_eq!(resolve(&v, "c", |s| &s.id.0).unwrap().id.0, "c");
        assert!(resolve(&v, "#4", |s| &s.id.0).is_none());
        assert!(resolve(&v, "#0", |s| &s.id.0).is_none());
    }

    #[test]
    fn flows_by_name_and_jobs_by_id() {
        let f = |n: &str| FlowSummary {
            name: n.into(),
            title: n.into(),
            origin: FlowOrigin::Project,
            active_runs: 0,
            source: None,
            bundle: None,
        };
        let flows_in = [f("weekly"), f("deploy")];
        assert_eq!(flows(&flows_in)[0].name, "deploy");
        let j = |id: &str, name: &str, created_ms: u64| Job {
            id: JobId(id.into()),
            name: name.into(),
            cron: "* * * * *".into(),
            kind: JobKind::Simple {
                prompt: String::new(),
                skills: vec![],
                project: None,
                scene: None,
                model: None,
            },
            enabled: true,
            catch_up: false,
            created_ms,
            guard: Default::default(),
            auto_disabled: None,
            effective_guard: None,
            next_fire_ms: None,
            last: None,
            running_ms: None,
            today_tokens: 0,
        };
        let jobs_in = [j("j00000001", "a", 2), j("jffffffff", "b", 1)];
        let names: Vec<&str> = jobs(&jobs_in).iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
    }
}
