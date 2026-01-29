use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use console::Term;

pub struct SpinnerGuard {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    term: Term,
}

impl Drop for SpinnerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = self.term.clear_line();
        let _ = self.term.write_str("\r");
    }
}

pub fn start_spinner(message: impl Into<String>) -> SpinnerGuard {
    let message = message.into();
    let term = Term::stderr();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let term_thread = term.clone();
    let handle = thread::spawn(move || {
        let frames = ['/', '-', '\\', '|'];
        let mut index = 0usize;
        while !stop_thread.load(Ordering::SeqCst) {
            let frame = frames[index % frames.len()];
            let _ = term_thread.write_str(&format!("\r{frame} {message}"));
            let _ = term_thread.flush();
            index = index.wrapping_add(1);
            thread::sleep(Duration::from_millis(120));
        }
    });

    SpinnerGuard {
        stop,
        handle: Some(handle),
        term,
    }
}
