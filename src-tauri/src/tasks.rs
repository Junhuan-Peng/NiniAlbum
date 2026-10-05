use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    error::{AppError, AppResult},
    models::{ExportSummary, ExportTask},
};

#[derive(Default)]
pub struct TaskControl {
    paused: Mutex<bool>,
    wake: Condvar,
    cancelled: AtomicBool,
}

impl TaskControl {
    pub fn checkpoint(&self) -> AppResult<()> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(AppError::Cancelled);
        }
        let mut paused = self.paused.lock().expect("task pause lock poisoned");
        while *paused && !self.cancelled.load(Ordering::SeqCst) {
            paused = self.wake.wait(paused).expect("task pause lock poisoned");
        }
        if self.cancelled.load(Ordering::SeqCst) {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn pause(&self) {
        *self.paused.lock().expect("task pause lock poisoned") = true;
    }

    fn resume(&self) {
        *self.paused.lock().expect("task pause lock poisoned") = false;
        self.wake.notify_all();
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

struct TaskRecord {
    snapshot: ExportTask,
    control: Arc<TaskControl>,
}

#[derive(Default)]
pub struct ExportTaskManager {
    records: Mutex<Vec<TaskRecord>>,
}

impl ExportTaskManager {
    pub fn create(
        &self,
        destination: &str,
        root: String,
    ) -> AppResult<(ExportTask, Arc<TaskControl>)> {
        let mut records = self.records.lock().expect("export task lock poisoned");
        if records.iter().any(|record| {
            record.snapshot.destination == destination
                && record.snapshot.root == root
                && is_active(&record.snapshot.status)
        }) {
            return Err(AppError::InvalidInput("该目标目录已有导出任务".into()));
        }
        if records.len() >= 50 {
            if let Some(index) = records
                .iter()
                .position(|record| !is_active(&record.snapshot.status))
            {
                records.remove(index);
            }
        }
        let snapshot = ExportTask {
            id: uuid::Uuid::new_v4().to_string(),
            destination: destination.into(),
            root,
            status: "queued".into(),
            completed_files: 0,
            total_files: 0,
            current_file: String::new(),
            created_at: now_millis(),
            summary: None,
            error: None,
        };
        let control = Arc::new(TaskControl::default());
        records.push(TaskRecord {
            snapshot: snapshot.clone(),
            control: Arc::clone(&control),
        });
        Ok((snapshot, control))
    }

    pub fn list(&self) -> Vec<ExportTask> {
        let mut tasks: Vec<_> = self
            .records
            .lock()
            .expect("export task lock poisoned")
            .iter()
            .map(|record| record.snapshot.clone())
            .collect();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        tasks
    }

    pub fn progress(
        &self,
        id: &str,
        completed_files: usize,
        total_files: usize,
        current_file: &str,
    ) -> Option<ExportTask> {
        self.update(id, |task| {
            if task.status != "paused" && task.status != "cancelling" {
                task.status = "running".into();
            }
            task.completed_files = completed_files;
            task.total_files = total_files;
            task.current_file = current_file.into();
        })
    }

    pub fn complete(&self, id: &str, summary: ExportSummary) -> Option<ExportTask> {
        self.update(id, |task| {
            task.status = "completed".into();
            task.completed_files = task.total_files;
            task.current_file.clear();
            task.summary = Some(summary);
        })
    }

    pub fn fail(&self, id: &str, error: String) -> Option<ExportTask> {
        self.update(id, |task| {
            task.status = "failed".into();
            task.current_file.clear();
            task.error = Some(error);
        })
    }

    pub fn cancelled(&self, id: &str) -> Option<ExportTask> {
        self.update(id, |task| {
            task.status = "cancelled".into();
            task.current_file.clear();
        })
    }

    pub fn pause(&self, id: &str) -> AppResult<ExportTask> {
        let mut records = self.records.lock().expect("export task lock poisoned");
        let record = records
            .iter_mut()
            .find(|record| record.snapshot.id == id)
            .ok_or_else(|| AppError::NotFound(format!("导出任务 {id}")))?;
        if !matches!(record.snapshot.status.as_str(), "queued" | "running") {
            return Err(AppError::InvalidInput("当前任务不能暂停".into()));
        }
        record.control.pause();
        record.snapshot.status = "paused".into();
        Ok(record.snapshot.clone())
    }

    pub fn resume(&self, id: &str) -> AppResult<ExportTask> {
        let mut records = self.records.lock().expect("export task lock poisoned");
        let record = records
            .iter_mut()
            .find(|record| record.snapshot.id == id)
            .ok_or_else(|| AppError::NotFound(format!("导出任务 {id}")))?;
        if record.snapshot.status != "paused" {
            return Err(AppError::InvalidInput("当前任务没有暂停".into()));
        }
        record.control.resume();
        record.snapshot.status = "running".into();
        Ok(record.snapshot.clone())
    }

    pub fn cancel(&self, id: &str) -> AppResult<ExportTask> {
        let mut records = self.records.lock().expect("export task lock poisoned");
        let record = records
            .iter_mut()
            .find(|record| record.snapshot.id == id)
            .ok_or_else(|| AppError::NotFound(format!("导出任务 {id}")))?;
        if !is_active(&record.snapshot.status) {
            return Err(AppError::InvalidInput("当前任务已经结束".into()));
        }
        record.control.cancel();
        record.snapshot.status = "cancelling".into();
        Ok(record.snapshot.clone())
    }

    pub fn clear_finished(&self) {
        self.records
            .lock()
            .expect("export task lock poisoned")
            .retain(|record| is_active(&record.snapshot.status));
    }

    fn update(&self, id: &str, update: impl FnOnce(&mut ExportTask)) -> Option<ExportTask> {
        let mut records = self.records.lock().expect("export task lock poisoned");
        let record = records.iter_mut().find(|record| record.snapshot.id == id)?;
        update(&mut record.snapshot);
        Some(record.snapshot.clone())
    }
}

fn is_active(status: &str) -> bool {
    matches!(status, "queued" | "running" | "paused" | "cancelling")
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_can_pause_resume_and_cancel() {
        let manager = ExportTaskManager::default();
        let (task, control) = manager.create("local", "/tmp/export".into()).unwrap();
        assert_eq!(manager.pause(&task.id).unwrap().status, "paused");
        assert_eq!(manager.resume(&task.id).unwrap().status, "running");
        assert_eq!(manager.cancel(&task.id).unwrap().status, "cancelling");
        assert!(matches!(control.checkpoint(), Err(AppError::Cancelled)));
    }
}
