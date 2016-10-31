use prelude::v1::*;
use base::*;
use shim::*;
use units::*;
use utils::*;
use isr::*;

unsafe impl Send for Task {}

/// Handle for a FreeRTOS task
pub struct Task {
    task_handle: FreeRtosTaskHandle,
}

/// Task's execution priority
#[derive(Debug, Copy, Clone)]
pub enum TaskPriority {
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

/// Notification to be sent to a task.
#[derive(Debug, Copy, Clone)]
pub enum TaskNotification {
    /// Send the event, unblock the task, the task's notification value isn't changed.
    NoAction,
    /// Perform a logical or with the task's notification value.
    SetBits(u32),
    /// Increment the task's notification value by one.
    Increment,
    /// Set the task's notification value to this value.
    OverwriteValue(u32),
    /// Try to set the task's notification value to this value. Succeeds
    /// only if the task has no pending notifications. Otherwise, the
    /// notification call will fail.
    SetValue(u32),
}

impl TaskNotification {
    fn to_freertos(&self) -> (u32, u8) {
        match *self {
            TaskNotification::NoAction => (0, 0),
            TaskNotification::SetBits(v) => (v, 1),
            TaskNotification::Increment => (0, 2),
            TaskNotification::OverwriteValue(v) => (v, 3),
            TaskNotification::SetValue(v) => (v, 4),
        }
    }
}

impl TaskPriority {
    fn to_freertos(&self) -> FreeRtosUBaseType {
        match *self {
            TaskPriority::BelowNormal => 6,
            TaskPriority::Normal => 5,
            TaskPriority::AboveNormal => 4,
            TaskPriority::High => 3,
        }
    }
}

/// Helper for spawning a new task. Instantiate with [`Task::new()`].
///
/// [`Task::new()`]: struct.Task.html#method.new
pub struct TaskBuilder {
    task_name: String,
    task_stack_size: u16,
    task_priority: TaskPriority,
}

impl TaskBuilder {
    /// Set the task's name.
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.task_name = name.into();
        self
    }

    /// Set the stack size, in words.
    pub fn stack_size(&mut self, stack_size: u16) -> &mut Self {
        self.task_stack_size = stack_size;
        self
    }

    /// Set the task's priority.
    pub fn priority(&mut self, priority: TaskPriority) -> &mut Self {
        self.task_priority = priority;
        self
    }

    /// Start a new task that can't return a value.
    pub fn start<F>(&self, func: F) -> Result<Task, FreeRtosError>
        where F: FnOnce() -> (),
              F: Send + 'static
    {

        Task::spawn(&self.task_name,
                    self.task_stack_size,
                    self.task_priority,
                    func)

    }
}

extern {
  pub fn console_putchar(char: i8);
}
pub fn message(msg: &str) {
    for char in msg.chars() {
        // Lossy converstion from unicode to ASCII
        let ascii_char = { if char > '\x7f' {'?'} else {char}};
        unsafe {
            console_putchar(ascii_char as i8);
        }
    }
}


impl Task {
    /// Prepare a builder object for the new task.
    pub fn new() -> TaskBuilder {
        TaskBuilder {
            task_name: "rust_task".into(),
            task_stack_size: 1024,
            task_priority: TaskPriority::Normal,
        }
    }

    unsafe fn spawn_inner<'a>(f: Box<FnBox() + Send + 'a>,
                              name: &str,
                              stack_size: u16,
                              priority: TaskPriority)
                              -> Result<Task, FreeRtosError> {
        let f = Box::new(f);
        let param_ptr = &*f as *const _ as *mut _;

        let (success, task_handle) = {
            let name = name.as_bytes();
            let name_len = name.len();
            let mut task_handle = mem::zeroed::<CVoid>();

            message("About to call freertos_rs_spawn_task\n");
            let ret = freertos_rs_spawn_task(thread_start,
                                             param_ptr,
                                             name.as_ptr(),
                                             name_len as u8,
                                             stack_size,
                                             priority.to_freertos(),
                                             &mut task_handle);

            (ret == 0, task_handle)
        };

        if success {
            mem::forget(f);
        } else {
            return Err(FreeRtosError::OutOfMemory);
        }

        extern "C" fn thread_start(main: *mut CVoid) -> *mut CVoid {
            unsafe {
                {
                    message("thread_start\n");
                    let b = Box::from_raw(main as *mut Box<FnBox()>);
                    b();
                }

                freertos_rs_delete_task(0 as *const _);
            }

            0 as *mut _
        }

        Ok(Task { task_handle: task_handle as usize as *const _ })
    }


    fn spawn<F>(name: &str,
                stack_size: u16,
                priority: TaskPriority,
                f: F)
                -> Result<Task, FreeRtosError>
        where F: FnOnce() -> (),
              F: Send + 'static
    {
        unsafe {
            return Task::spawn_inner(Box::new(f), name, stack_size, priority);
        }
    }


    /// Get the name of the current task.
    pub fn get_name(&self) -> Result<String, ()> {
        unsafe {
            let name_ptr = freertos_rs_task_get_name(self.task_handle);
            let name = str_from_c_string(name_ptr);
            if let Ok(name) = name {
                return Ok(name);
            }

            Err(())
        }
    }

    /// Try to find the task of the current execution context.
    pub fn current() -> Result<Task, FreeRtosError> {
        unsafe {
            let t = freertos_rs_get_current_task();
            if t != 0 as *const _ {
                Ok(Task { task_handle: t })
            } else {
                Err(FreeRtosError::TaskNotFound)
            }
        }
    }

    /// Forcibly set the notification value for this task.
    pub fn set_notification_value(&self, val: u32) {
        self.notify(TaskNotification::OverwriteValue(val))
    }

    /// Notify this task.
    pub fn notify(&self, notification: TaskNotification) {
        unsafe {
            let n = notification.to_freertos();
            freertos_rs_task_notify(self.task_handle, n.0, n.1);
        }
    }

    /// Notify this task from an interrupt.
    pub fn notify_from_isr(&self,
                           context: &InterruptContext,
                           notification: TaskNotification)
                           -> Result<(), FreeRtosError> {
        unsafe {
            let n = notification.to_freertos();
            let t = freertos_rs_task_notify_isr(self.task_handle,
                                                n.0,
                                                n.1,
                                                context.get_task_field_mut());
            if t != 0 {
                Err(FreeRtosError::QueueFull)
            } else {
                Ok(())
            }
        }
    }

    /// Take the notification and either clear the notification value or decrement it by one.
    pub fn take_notification(&self, clear: bool, wait_for: Duration) -> u32 {
        unsafe { freertos_rs_task_notify_take(if clear { 1 } else { 0 }, wait_for.to_ticks()) }
    }

    /// Wait for a notification to be posted.
    pub fn wait_for_notification(&self,
                                 clear_bits_enter: u32,
                                 clear_bits_exit: u32,
                                 wait_for: Duration)
                                 -> Result<u32, FreeRtosError> {
        unsafe {
            let mut val = 0;
            let r = freertos_rs_task_notify_wait(clear_bits_enter,
                                                 clear_bits_exit,
                                                 &mut val as *mut _,
                                                 wait_for.to_ticks());

            if r == 0 {
                Ok(val)
            } else {
                Err(FreeRtosError::Timeout)
            }
        }
    }
}

/// Helper methods to be performed on the task that is currently executing.
pub struct CurrentTask;
impl CurrentTask {
    pub fn get_tick_count() -> FreeRtosTickType {
        unsafe { freertos_rs_xTaskGetTickCount() }
    }

    /// Delay the execution of the current task.
    pub fn delay(delay: Duration) {
        unsafe {
            freertos_rs_vTaskDelay(delay.to_ticks());
        }
    }
}
