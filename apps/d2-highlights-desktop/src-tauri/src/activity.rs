use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Activity {
    Analysis,
    Render,
    Update,
    Save,
    Export,
}

#[derive(Default)]
struct Runtime {
    active: Option<Activity>,
    closing: bool,
    exit_ready: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Activities(Arc<(Mutex<Runtime>, Condvar)>);

pub(crate) struct ActivityLease {
    owner: Activities,
}

impl Activities {
    pub(crate) fn begin(&self, activity: Activity) -> Result<ActivityLease, String> {
        let mut runtime = self
            .0
            .0
            .lock()
            .map_err(|_| "任务状态不可用，请重启应用。")?;
        if runtime.closing {
            return Err("正在保存并关闭应用，请等待任务收尾。".into());
        }
        if let Some(active) = runtime.active {
            return Err(match active {
                Activity::Update => "更新正在下载或安装，请等待完成。",
                Activity::Render => "成片任务正在运行，请等待完成或先取消。",
                Activity::Analysis => "录像正在分析，请等待完成。",
                Activity::Save => "剪辑方案正在保存，请稍后重试。",
                Activity::Export => "文件正在导出，请等待完成。",
            }
            .into());
        }
        runtime.active = Some(activity);
        Ok(ActivityLease {
            owner: self.clone(),
        })
    }

    pub(crate) fn begin_close(&self) -> Result<(), String> {
        let mut runtime = self
            .0
            .0
            .lock()
            .map_err(|_| "任务状态不可用，请重启应用。")?;
        if runtime.active == Some(Activity::Update) {
            return Err("更新正在下载或安装，完成前不能关闭应用。".into());
        }
        if runtime.closing {
            return Err("应用正在等待任务收尾，请稍候。".into());
        }
        runtime.closing = true;
        Ok(())
    }

    pub(crate) fn wait_for_close(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut runtime = self
            .0
            .0
            .lock()
            .map_err(|_| "任务状态不可用，请重启应用。")?;
        while runtime.active.is_some() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                runtime.closing = false;
                return Err("任务仍在收尾，应用已保留打开状态，请稍后再关闭。".into());
            }
            let (next, _) = self
                .0
                .1
                .wait_timeout(runtime, remaining)
                .map_err(|_| "等待任务收尾失败，应用保持打开。")?;
            runtime = next;
        }
        if !runtime.closing {
            return Err("关闭请求已失效。".into());
        }
        runtime.exit_ready = true;
        Ok(())
    }

    pub(crate) fn exit_ready(&self) -> bool {
        self.0
            .0
            .lock()
            .map(|runtime| runtime.exit_ready)
            .unwrap_or(false)
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.0
            .0
            .lock()
            .map(|runtime| runtime.closing)
            .unwrap_or(true)
    }

    pub(crate) fn set_update_exit_ready(&self, ready: bool) -> Result<(), String> {
        let mut runtime = self.0.0.lock().map_err(|_| "更新退出状态不可用。")?;
        if runtime.active != Some(Activity::Update) || runtime.closing {
            return Err("未取得独占更新权限，不能安装。".into());
        }
        runtime.exit_ready = ready;
        Ok(())
    }
}

impl Drop for ActivityLease {
    fn drop(&mut self) {
        let mut runtime = self
            .owner
            .0
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if runtime.active == Some(Activity::Update) {
            runtime.exit_ready = false;
        }
        runtime.active = None;
        self.owner.0.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{Activities, Activity};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    #[test]
    fn update_and_work_are_mutually_exclusive_in_both_orders() {
        for work in [
            Activity::Analysis,
            Activity::Render,
            Activity::Save,
            Activity::Export,
        ] {
            let state = Activities::default();
            let working = state.begin(work).unwrap();
            assert!(state.begin(Activity::Update).is_err());
            drop(working);
            let updating = state.begin(Activity::Update).unwrap();
            assert!(state.begin(work).is_err());
            assert!(state.begin_close().is_err());
            drop(updating);
            assert!(state.begin(work).is_ok());
        }
    }

    #[test]
    fn racing_update_and_render_have_only_one_winner() {
        let state = Activities::default();
        let barrier = Arc::new(Barrier::new(3));
        let threads: Vec<_> = [Activity::Update, Activity::Render]
            .into_iter()
            .map(|kind| {
                let state = state.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let lease = state.begin(kind);
                    barrier.wait();
                    lease
                })
            })
            .collect();
        barrier.wait();
        barrier.wait();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    }

    #[test]
    fn close_waits_for_worker_drop_and_blocks_new_work() {
        let state = Activities::default();
        let lease = state.begin(Activity::Render).unwrap();
        state.begin_close().unwrap();
        assert!(!state.exit_ready());
        assert!(state.begin(Activity::Analysis).is_err());
        let waiting = state.clone();
        let join = std::thread::spawn(move || waiting.wait_for_close(Duration::from_secs(2)));
        drop(lease);
        join.join().unwrap().unwrap();
        assert!(state.exit_ready());
        assert!(state.begin(Activity::Render).is_err());
    }

    #[test]
    fn timed_out_close_keeps_app_open_and_releases_closing_gate() {
        let state = Activities::default();
        let lease = state.begin(Activity::Analysis).unwrap();
        state.begin_close().unwrap();
        assert!(state.wait_for_close(Duration::ZERO).is_err());
        assert!(!state.exit_ready());
        drop(lease);
        assert!(state.begin(Activity::Render).is_ok());
    }

    #[test]
    fn only_exclusive_update_can_prepare_installer_exit() {
        let state = Activities::default();
        assert!(state.set_update_exit_ready(true).is_err());
        let _update = state.begin(Activity::Update).unwrap();
        state.set_update_exit_ready(true).unwrap();
        assert!(state.exit_ready());
        state.set_update_exit_ready(false).unwrap();
        assert!(!state.exit_ready());
    }
}
