use super::{unsupported, Void};
use crate::ffi::CStr;
use crate::io;
use crate::time::Duration;

pub struct Thread(Void);

pub const DEFAULT_MIN_STACK_SIZE: usize = 4096;

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(_stack: usize, _p: Box<dyn FnOnce()>) -> io::Result<Thread> {
        unsupported()
    }

    pub fn yield_now() {
        // do nothing
    }

    pub fn set_name(_name: &CStr) {
        // nope
    }

    pub fn sleep(_dur: Duration) {
        panic!("can't sleep");
    }

    pub fn join(self) {
        match self.0 {}
    }
}

pub mod guard {
    pub type Guard = !;
    pub unsafe fn current() -> Option<Guard> {
        None
    }
    pub unsafe fn init() -> Option<Guard> {
        None
    }
}

pub fn my_id() -> u32 {
    if let Ok(xous::Result::ThreadID(tid)) = xous::current_tid() {
        tid as u32
    } else {
        0
    }
}
