use crate::engines::{module_engine::sword_module::module::SwordModule, xbible_engine::engine::XBibleEngine};

#[derive(Debug, Clone, uniffi::Enum)]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    Failed { error: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: TaskState,
    pub progress: f64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct TaskData {
    pub(crate) status: TaskStatus,
    pub(crate) result_modules: Vec<SwordModule>,
}

#[uniffi::export]
impl XBibleEngine {
    /// Cancel a background task
    pub fn cancel_task(&self, task_id: String) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(task) = tasks.get_mut(&task_id) {
            if matches!(task.status.state, TaskState::Queued | TaskState::Running) {
                task.status.state = TaskState::Failed { error: "Cancelled".to_string() };
                task.status.message = "Task cancelled".to_string();
            }
        }
    }

    /// Get the status of a background task
    pub fn get_task_status(&self, task_id: String) -> Option<TaskStatus> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).map(|t| t.status.clone())
    }

    /// Get the modules resulting from a fetch task
    pub fn get_task_result_modules(&self, task_id: String) -> Vec<SwordModule> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).map(|t| t.result_modules.clone()).unwrap_or_default()
    }

}