//! 流程包建议的作业（flows.md §7.1、§7.12）：`bundle.yaml` 的 `jobs` 换成项目 `.conix/jobs.json` 里的样子，
//! 装包时由命令行问过人再加进项目。加进去的一律停用：和克隆下来的仓库带着的作业一样，在这台机器上要人启用才跑。
//! 流程库只看形状（没有 cron 解析）；cron 能不能解析、护栏的值对不对在这里判，校验包时（`package::validate_bundle`）
//! 就判，所以带着坏作业的包装不进库也装不进项目。

use std::collections::BTreeSet;
use std::path::Path;

use client::ProjectState;
use harnix_flow::bundle::{self, BundleMeta, JobKindDecl};
use port::{Job, JobGuard, JobId, JobKind};

use super::{JobStore, cron};

/// 一个包建议的作业，换成项目里的样子（停用）；换不成的每条一句为什么。形状的错（流程库的
/// `jobs_problems`）不在这里重报：流程库校验每个流程时已经带上了。
pub fn jobs_of(meta: &BundleMeta, stems: &BTreeSet<String>) -> (Vec<Job>, Vec<String>) {
    let mut problems = vec![];
    let mut out = vec![];
    for (i, j) in meta.jobs.iter().enumerate() {
        let at = format!("{}: jobs[{i}]", bundle::BUNDLE_FILE);
        if let Err(e) = cron::validate(&j.cron) {
            problems.push(format!("{at}: cron {:?}: {e}", j.cron));
        }
        let guard = if j.guard.is_null() {
            JobGuard::default()
        } else {
            match serde_json::from_value::<JobGuard>(j.guard.clone()) {
                Ok(g) => g,
                Err(e) => {
                    problems.push(format!("{at}: guard: {e}"));
                    continue;
                }
            }
        };
        let kind = match &j.kind {
            JobKindDecl::Simple(s) => JobKind::Simple {
                prompt: s.prompt.clone(),
                skills: s.skills.clone(),
                project: None,
                scene: None,
                model: s.model.clone(),
            },
            JobKindDecl::Flow(f) => {
                let Some(flow) = j.flow_of(&meta.name, stems) else {
                    continue;
                };
                JobKind::Flow {
                    flow,
                    subject: if f.subject.trim().is_empty() {
                        j.name.clone()
                    } else {
                        f.subject.clone()
                    },
                    inputs: f.inputs.clone(),
                }
            }
        };
        out.push(Job {
            id: JobId(j.id.clone()),
            name: j.name.clone(),
            cron: j.cron.clone(),
            kind,
            enabled: false,
            catch_up: j.catch_up,
            created_ms: 0,
            guard,
            auto_disabled: None,
            effective_guard: None,
            next_fire_ms: None,
            last: None,
            running_ms: None,
            today_tokens: 0,
        });
    }
    (out, problems)
}

/// 包目录里建议的作业；`bundle.yaml` 读不出来就没有（那是包校验另报的错）。
pub fn jobs_in(dir: &Path) -> (Vec<Job>, Vec<String>) {
    match bundle::meta(dir) {
        Ok(meta) => jobs_of(&meta, &bundle::flow_stems(dir)),
        Err(_) => (vec![], vec![]),
    }
}

/// 装包时给人看的：要加的，与项目里已有同 ID、不加的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offer {
    pub new: Vec<Job>,
    pub existing: Vec<Job>,
}

impl Offer {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty() && self.existing.is_empty()
    }
}

/// 命令行直接读写项目的作业定义，当受信：是人在终端里让它做的，加进去的又都停着。实例里那份由信任管着，
/// 它重读时照旧按信任来。
fn open(project: &Path) -> JobStore {
    let mut s = JobStore::open_in(&ProjectState::new(project));
    s.set_trusted(true);
    s
}

/// 项目里现在的作业。
pub fn project_jobs(project: &Path) -> Vec<Job> {
    open(project).list(0)
}

/// `jobs` 里哪些要加、哪些项目里已有同 ID（已有的从不覆盖）。`skip` 是不再提的 ID：更新时旧版已经提过的。
pub fn offer(project: &Path, jobs: Vec<Job>, skip: &BTreeSet<String>) -> Offer {
    let have = project_jobs(project);
    let mut o = Offer::default();
    for j in jobs.into_iter().filter(|j| !skip.contains(j.id.as_str())) {
        match have.iter().find(|h| h.id == j.id) {
            Some(h) => o.existing.push(h.clone()),
            None => o.new.push(j),
        }
    }
    o
}

/// 把作业加进项目，停用；加之前再看一眼，这期间有了同 ID 的就跳过。回加了的。
pub fn add(project: &Path, jobs: &[Job], now_ms: u64) -> Result<Vec<JobId>, String> {
    let mut store = open(project);
    let mut added = vec![];
    for j in jobs {
        if store.get(j.id.as_str(), now_ms).is_some() {
            continue;
        }
        let mut j = j.clone();
        j.enabled = false;
        added.push(j.id.clone());
        store.upsert(j, now_ms)?;
    }
    Ok(added)
}

/// 项目里起这个包的流程的作业（`<包>` 或 `<包>/…`）：删包不删它们，只提醒。
pub fn referencing(project: &Path, bundle_name: &str) -> Vec<Job> {
    project_jobs(project)
        .into_iter()
        .filter(|j| match &j.kind {
            JobKind::Flow { flow, .. } => {
                flow == bundle_name || flow.starts_with(&format!("{bundle_name}/"))
            }
            JobKind::Simple { .. } => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(dir: &Path, jobs: &str) -> std::path::PathBuf {
        let b = dir.join("dev");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            b.join("bundle.yaml"),
            format!("name: dev\nflows: [main]\njobs:\n{jobs}"),
        )
        .unwrap();
        std::fs::write(
            b.join("main.yaml"),
            "name: main\nstations:\n  - {id: a, exit: [{kind: confirm}]}\n",
        )
        .unwrap();
        b
    }

    #[test]
    fn suggested_jobs_become_disabled_project_jobs_and_existing_ids_are_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let b = bundle(
            d.path(),
            "  - {id: daily, name: 每日巡检, cron: \"0 9 * * *\", kind: {simple: {prompt: 看看}}}\n  - {id: weekly, name: 周报, cron: \"0 10 * * 1\", kind: {flow: {flow: dev}}, guard: {overlap: skip}}\n",
        );
        let (jobs, problems) = jobs_in(&b);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(jobs.len(), 2);
        let JobKind::Flow { flow, subject, .. } = &jobs[1].kind else {
            panic!()
        };
        assert_eq!((flow.as_str(), subject.as_str()), ("dev/main", "周报"));
        assert_eq!(jobs[1].guard.overlap, Some(port::Overlap::Skip));

        let project = tempfile::tempdir().unwrap();
        let p = project.path();
        let mut mine = jobs[0].clone();
        mine.name = "我们自己的".into();
        add(p, &[mine], 1).unwrap();
        let o = offer(p, jobs.clone(), &BTreeSet::new());
        assert_eq!(o.new.len(), 1);
        assert_eq!(o.existing[0].name, "我们自己的");
        assert_eq!(add(p, &jobs, 2).unwrap(), vec![JobId("weekly".into())]);
        let all = project_jobs(p);
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|j| !j.enabled));
        assert_eq!(all[0].name, "我们自己的", "not overwritten");
        let skip: BTreeSet<String> = ["weekly".to_string()].into();
        assert!(offer(p, jobs, &skip).new.is_empty());
        assert_eq!(referencing(p, "dev").len(), 1);
        assert!(referencing(p, "de").is_empty());
    }

    #[test]
    fn a_cron_that_does_not_parse_or_an_odd_guard_is_a_problem() {
        let d = tempfile::tempdir().unwrap();
        let b = bundle(
            d.path(),
            "  - {id: a, name: a, cron: \"99 * * * *\", kind: {simple: {prompt: x}}}\n  - {id: b, name: b, cron: \"* * * * *\", kind: {simple: {prompt: x}}, guard: {overlap: sometimes}}\n",
        );
        let (_, problems) = jobs_in(&b);
        let p = problems.join("\n");
        assert!(
            p.contains("jobs[0]: cron") && p.contains("jobs[1]: guard"),
            "{p}"
        );
    }
}
