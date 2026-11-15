//! PROTOTYPE (K3): the opt-in blocking runner, outside the gate (std). The host registers it:
//! `caps.blocking_runner(ThreadRunner::new(4).runner())`. A bounded pool of worker threads
//! with a FIFO queue; jobs run once, in submission order per worker. Nothing here is reachable
//! from the gated core, which only knows the `Runner` type.
use cb_core::b1::Runner;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send>;
struct Q {
    jobs: Mutex<(VecDeque<Job>, bool)>,
    cv: Condvar,
}
pub struct ThreadRunner {
    q: Arc<Q>,
    workers: Vec<JoinHandle<()>>,
}
impl ThreadRunner {
    pub fn new(n: usize) -> Self {
        let q = Arc::new(Q { jobs: Mutex::new((VecDeque::new(), false)), cv: Condvar::new() });
        let workers = (0..n)
            .map(|_| {
                let q = q.clone();
                std::thread::spawn(move || loop {
                    let job = {
                        let mut g = q.jobs.lock().unwrap();
                        loop {
                            if let Some(j) = g.0.pop_front() {
                                break Some(j);
                            }
                            if g.1 {
                                break None;
                            }
                            g = q.cv.wait(g).unwrap();
                        }
                    };
                    match job {
                        Some(j) => j(),
                        None => return,
                    }
                })
            })
            .collect();
        ThreadRunner { q, workers }
    }
    /// The capability value to hand to `Caps1::blocking_runner`.
    pub fn runner(&self) -> Runner {
        let q = self.q.clone();
        Arc::new(move |job| {
            q.jobs.lock().unwrap().0.push_back(job);
            q.cv.notify_one();
        })
    }
}
impl Drop for ThreadRunner {
    fn drop(&mut self) {
        self.q.jobs.lock().unwrap().1 = true;
        self.q.cv.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}
