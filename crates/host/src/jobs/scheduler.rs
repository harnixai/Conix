//! 调度循环：算出最早的触发时刻、睡到那一刻（或被改动唤醒）、触发到期的作业。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use port::{Job, JobEntry};
use tokio::sync::Notify;

use super::store::JobStore;
use crate::sync::Locked;

/// 触发一次作业。`fired_ms` 同时是触发记录的键，`Fire` 的实现负责写 `Running` 行和之后的结局行。
/// `scheduled` 是到点触发（受重叠、并发、每日额度管）；人"立即运行"的不是。
#[async_trait]
pub trait Fire: Send + Sync + 'static {
    async fn fire(&self, job: Job, fired_ms: u64, scheduled: bool);
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

const MAX_SLEEP_MS: u64 = 60_000;

/// 调度器句柄，可克隆。存储由调用方持有一份 `Arc<Mutex<_>>`，`Fire` 的实现要写历史也用同一份。
#[derive(Clone)]
pub struct Scheduler {
    store: Arc<Mutex<JobStore>>,
    clock: Clock,
    fire: Arc<dyn Fire>,
    wake: Arc<Notify>,
    stopped: Arc<AtomicBool>,
}

impl Scheduler {
    /// 启动后台任务（需在 tokio 运行时内调用），先做一次补跑再进入循环。
    pub fn start(store: Arc<Mutex<JobStore>>, clock: Clock, fire: Arc<dyn Fire>) -> Scheduler {
        let s = Scheduler {
            store,
            clock,
            fire,
            wake: Arc::new(Notify::new()),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        tokio::spawn(s.clone().run());
        s
    }

    pub fn store(&self) -> Arc<Mutex<JobStore>> {
        self.store.clone()
    }

    pub fn now(&self) -> u64 {
        (self.clock)()
    }

    fn lock(&self) -> MutexGuard<'_, JobStore> {
        // 持锁线程 panic 只会让内存与磁盘暂时不一致，不值得把整个调度器一起带崩。
        self.store.locked()
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        // notify_one 会存一个许可：即使循环此刻不在等待，下次也会立刻醒。
        self.wake.notify_one();
    }

    pub fn list(&self) -> Vec<Job> {
        self.lock().list(self.now())
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.lock().get(id, self.now())
    }

    pub fn history(&self, id: &str, limit: usize) -> Vec<JobEntry> {
        self.lock().history(id, limit)
    }

    pub fn upsert(&self, job: Job) -> Result<Job, String> {
        let r = self.lock().upsert(job, self.now())?;
        self.wake.notify_one();
        Ok(r)
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let r = self.lock().delete(id)?;
        self.wake.notify_one();
        Ok(r)
    }

    /// 项目的信任变了：重读作业定义。新读进来的作业按启动时的规矩处理到期的触发（`catch_up` 的补一次，
    /// 其余只前移基线），否则信任一给，停着期间"错过"的触发会一下子全跑起来。返回此前有、现在没有的作业。
    pub fn set_trusted(&self, trusted: bool) -> Vec<port::JobId> {
        let before: Vec<port::JobId> = self.list().into_iter().map(|j| j.id).collect();
        self.lock().set_trusted(trusted);
        let now = self.now();
        for job in self.take_due(now, true) {
            self.spawn_fire(job, now, true);
        }
        self.wake.notify_one();
        let after = self.list();
        before
            .into_iter()
            .filter(|id| !after.iter().any(|j| j.id == *id))
            .collect()
    }

    /// 定义在实例之外变了（命令行加了作业）：按此刻的信任重读，规矩同 [`Scheduler::set_trusted`]。
    pub fn reload(&self) -> Vec<port::JobId> {
        let trusted = self.lock().trusted();
        self.set_trusted(trusted)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool, String> {
        let r = self.lock().set_enabled(id, enabled, self.now())?;
        self.wake.notify_one();
        Ok(r)
    }

    /// 立刻触发，不看 `enabled`，也不动调度基线（手动跑一次不该吞掉下一次定时触发）。
    pub fn run_now(&self, id: &str) -> Result<u64, String> {
        self.run_now_with(id, |_| {})
    }

    /// 立即跑一次，这一次的作业先经 `edit` 改一下（换主题、补一句），定义不变。
    pub fn run_now_with(&self, id: &str, edit: impl FnOnce(&mut Job)) -> Result<u64, String> {
        let now = self.now();
        let mut job = self
            .lock()
            .get(id, now)
            .ok_or_else(|| format!("no job {id}"))?;
        edit(&mut job);
        self.spawn_fire(job, now, false);
        Ok(now)
    }

    fn spawn_fire(&self, job: Job, fired_ms: u64, scheduled: bool) {
        // 另起任务：一次触发要等一整轮对话，不能让它挡住调度循环。
        let fire = self.fire.clone();
        tokio::spawn(async move { fire.fire(job, fired_ms, scheduled).await });
    }

    /// 取出到期的作业并先落盘 `last_fired_ms`，再返回交给调用方触发；
    /// 崩溃发生在触发中途时，重启不会再触发一次。
    /// 到期但不该触发的（启动时 `catch_up == false`）只前移基线。
    fn take_due(&self, now: u64, startup: bool) -> Vec<Job> {
        let mut store = self.lock();
        let due: Vec<(String, bool)> = store
            .stored()
            .iter()
            .filter(|s| s.due_ms().is_some_and(|t| t <= now))
            .map(|s| (s.job.id.0.clone(), !startup || s.job.catch_up))
            .collect();
        let mut jobs = Vec::new();
        for (id, fire) in due {
            if let Err(e) = store.mark_fired(&id, now) {
                tracing::warn!("作业 {id} 记录触发时刻失败：{e}");
            }
            // 落盘失败也照常触发：宁可内存里记着，也不能因为磁盘问题让作业不跑。
            if fire && let Some(j) = store.get(&id, now) {
                jobs.push(j);
            }
        }
        jobs
    }

    fn earliest(&self) -> Option<u64> {
        self.lock().stored().iter().filter_map(|s| s.due_ms()).min()
    }

    async fn run(self) {
        let mut startup = true;
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            let now = self.now();
            for job in self.take_due(now, startup) {
                self.spawn_fire(job, now, true);
            }
            startup = false;

            match self.earliest() {
                Some(t) => {
                    // tokio 的时钟是单调钟，机器睡眠时不走：一口气睡到点，醒来后会晚触发
                    // 睡眠那么久。每次至多睡一分钟、醒来按墙上时间重算。
                    let wait = t.saturating_sub(self.now()).min(MAX_SLEEP_MS);
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
                None => self.wake.notified().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::cron::next_after;
    use port::{JobId, JobKind};
    use std::sync::atomic::AtomicU64;

    const T0: u64 = 1_704_067_200_000; // 整分
    const MIN: u64 = 60_000;

    type Log = Arc<Mutex<Vec<(String, u64)>>>;

    struct Rec(Log);

    #[async_trait]
    impl Fire for Rec {
        async fn fire(&self, job: Job, fired_ms: u64, _scheduled: bool) {
            self.0.lock().unwrap().push((job.id.0, fired_ms));
        }
    }

    fn job(id: &str, cron: &str, catch_up: bool) -> Job {
        Job {
            id: JobId(id.into()),
            name: id.into(),
            cron: cron.into(),
            kind: JobKind::Simple {
                prompt: "hi".into(),
                skills: vec![],
                project: None,
                scene: None,
                model: None,
            },
            enabled: true,
            catch_up,
            created_ms: 0,
            guard: Default::default(),
            auto_disabled: None,
            effective_guard: None,
            next_fire_ms: None,
            last: None,
            running_ms: None,
            today_tokens: 0,
        }
    }

    struct Rig {
        sched: Scheduler,
        log: Log,
        clock: Arc<AtomicU64>,
        dir: tempfile::TempDir,
    }

    fn store_at(dir: &std::path::Path) -> JobStore {
        let mut s = JobStore::open(
            dir.join("jobs.json"),
            dir.join("jobs-state.json"),
            dir.join("jobs"),
        );
        s.set_trusted(true);
        s
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    async fn rig_with(dir: tempfile::TempDir, now: u64) -> Rig {
        let clock = Arc::new(AtomicU64::new(now));
        let c = clock.clone();
        let log: Log = Arc::default();
        let store = Arc::new(Mutex::new(store_at(dir.path())));
        let sched = Scheduler::start(
            store,
            Arc::new(move || c.load(Ordering::SeqCst)),
            Arc::new(Rec(log.clone())),
        );
        settle().await;
        Rig {
            sched,
            log,
            clock,
            dir,
        }
    }

    async fn rig() -> Rig {
        rig_with(tempfile::tempdir().unwrap(), T0).await
    }

    impl Rig {
        async fn advance(&self, ms: u64) {
            self.clock.fetch_add(ms, Ordering::SeqCst);
            tokio::time::advance(Duration::from_millis(ms)).await;
            settle().await;
        }

        fn fired(&self) -> Vec<(String, u64)> {
            self.log.lock().unwrap().clone()
        }
    }

    /// 命令行在实例之外加了作业（装包时建议的）：重读就看得到，停着的不跑。
    #[tokio::test(start_paused = true)]
    async fn reload_reads_jobs_added_outside_the_instance() {
        let r = rig().await;
        r.sched.upsert(job("a", "* * * * *", false)).unwrap();
        let mut other = store_at(r.dir.path());
        let mut b = job("b", "* * * * *", false);
        b.enabled = false;
        other.upsert(b, T0).unwrap();
        assert_eq!(r.sched.list().len(), 1);
        assert!(r.sched.reload().is_empty());
        let ids: Vec<String> = r.sched.list().into_iter().map(|j| j.id.0).collect();
        assert_eq!(ids, ["a", "b"]);
        r.advance(MIN).await;
        assert_eq!(r.fired(), vec![("a".into(), T0 + MIN)]);
    }

    #[tokio::test(start_paused = true)]
    async fn fires_at_next_minute_and_rearms() {
        let r = rig().await;
        r.sched.upsert(job("a", "* * * * *", false)).unwrap();
        settle().await;
        r.advance(MIN - 1).await;
        assert!(r.fired().is_empty());
        r.advance(1).await;
        assert_eq!(r.fired(), vec![("a".into(), T0 + MIN)]);
        r.advance(MIN).await;
        assert_eq!(r.fired().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn upsert_rearms_immediately() {
        let r = rig().await;
        r.sched.upsert(job("a", "0 0 1 1 *", false)).unwrap();
        settle().await;
        r.advance(MIN).await;
        assert!(r.fired().is_empty());
        // 改成每分钟：睡眠被唤醒并按新表达式重算。
        r.sched.upsert(job("a", "* * * * *", false)).unwrap();
        settle().await;
        r.advance(MIN).await;
        assert_eq!(r.fired().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_does_not_fire_and_reenable_has_no_backlog() {
        let r = rig().await;
        r.sched.upsert(job("a", "* * * * *", true)).unwrap();
        settle().await;
        r.sched.set_enabled("a", false).unwrap();
        settle().await;
        r.advance(5 * MIN).await;
        assert!(r.fired().is_empty());
        r.sched.set_enabled("a", true).unwrap();
        settle().await;
        assert!(r.fired().is_empty());
        r.advance(MIN).await;
        assert_eq!(r.fired().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn delete_stops_firing() {
        let r = rig().await;
        r.sched.upsert(job("a", "* * * * *", false)).unwrap();
        settle().await;
        assert!(r.sched.delete("a").unwrap());
        settle().await;
        r.advance(2 * MIN).await;
        assert!(r.fired().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn run_now_ignores_enabled_and_schedule() {
        let r = rig().await;
        r.sched.upsert(job("a", "0 0 1 1 *", false)).unwrap();
        r.sched.set_enabled("a", false).unwrap();
        settle().await;
        assert_eq!(r.sched.run_now("a").unwrap(), T0);
        settle().await;
        assert_eq!(r.fired(), vec![("a".into(), T0)]);
        assert!(r.sched.run_now("nope").is_err());
    }

    async fn restart_after_downtime(catch_up: bool) -> Vec<(String, u64)> {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = store_at(dir.path());
            s.upsert(job("a", "* * * * *", catch_up), T0).unwrap();
        }
        // 停机 10 分钟，错过 10 次。
        let r = rig_with(dir, T0 + 10 * MIN + 5_000).await;
        let fired = r.fired();
        // 补跑之后回到正常节奏。
        r.advance(MIN).await;
        let all = r.fired();
        assert_eq!(all.len(), fired.len() + 1);
        assert_eq!(all.last().unwrap().1, T0 + 11 * MIN + 5_000);
        let _ = &r.dir;
        fired
    }

    #[tokio::test(start_paused = true)]
    async fn catch_up_fires_exactly_once() {
        let fired = restart_after_downtime(true).await;
        assert_eq!(fired, vec![("a".into(), T0 + 10 * MIN + 5_000)]);
    }

    #[tokio::test(start_paused = true)]
    async fn no_catch_up_just_moves_on() {
        assert!(restart_after_downtime(false).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn catch_up_persists_last_fired() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = store_at(dir.path());
            s.upsert(job("a", "* * * * *", true), T0).unwrap();
        }
        let r = rig_with(dir, T0 + 3 * MIN).await;
        assert_eq!(r.fired().len(), 1);
        let reopened = store_at(r.dir.path());
        assert_eq!(reopened.stored()[0].last_fired_ms, Some(T0 + 3 * MIN));
        assert_eq!(next_after("* * * * *", T0 + 3 * MIN), Some(T0 + 4 * MIN));
    }
}
