//! Dropping what the UI thread replaced, somewhere other than the UI thread.
//!
//! Freeing is not free. The composites a rebuild hands the editor are a map of
//! tens of thousands of resolved glyphs, each with grids and name strings of
//! its own, and the old map was dropped in the frame that installed the new
//! one: 56 ms of one frame on a fast machine, the whole of a stall the user
//! sees as the editor catching on a keystroke. Nothing reads a value once it
//! has been replaced, so where it is freed is nobody's business but the
//! frame's — [`discard`] hands it to one long-lived thread that does nothing
//! else.
//!
//! One thread, not one per value: a spawn costs more than most of what is
//! discarded, and the order of the frees does not matter.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc;

type Garbage = Box<dyn Send>;

fn sender() -> &'static Mutex<mpsc::Sender<Garbage>> {
    static TX: OnceLock<Mutex<mpsc::Sender<Garbage>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Garbage>();
        std::thread::Builder::new()
            .name("discard".into())
            .spawn(move || {
                for garbage in rx {
                    drop(garbage)
                }
            })
            .expect("spawning the discard thread");
        Mutex::new(tx)
    })
}

/// Drops `value` on the discard thread. Falls back to dropping it here if that
/// thread is gone, which only a panic inside some `Drop` could have caused.
pub(crate) fn discard<T: Send + 'static>(value: T) {
    let garbage: Garbage = Box::new(value);
    let failed = sender().lock().unwrap().send(garbage);
    drop(failed);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Signal(mpsc::Sender<std::thread::ThreadId>);
    impl Drop for Signal {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    #[test]
    fn a_discarded_value_is_dropped_on_another_thread() {
        let (tx, rx) = mpsc::channel();
        discard(Signal(tx));
        let dropped_on = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the value is dropped");
        assert_ne!(dropped_on, std::thread::current().id());
    }
}
