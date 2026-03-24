/// Pre-allocated buffers for A* search to avoid per-request allocation.
pub struct AstarBuffers {
    pub g_score: Vec<f32>,
    pub came_from: Vec<u32>,
    pub closed: Vec<bool>,
}

impl AstarBuffers {
    pub fn new(num_nodes: usize) -> Self {
        Self {
            g_score: vec![f32::MAX; num_nodes],
            came_from: vec![u32::MAX; num_nodes],
            closed: vec![false; num_nodes],
        }
    }

    pub fn reset(&mut self) {
        self.g_score.fill(f32::MAX);
        self.came_from.fill(u32::MAX);
        self.closed.fill(false);
    }
}

/// Pool of reusable A* buffer sets.
/// Uses a simple Mutex<Vec> so multiple callers can acquire concurrently
/// without serializing behind a channel receiver lock.
pub struct AstarPool {
    buffers: std::sync::Mutex<Vec<AstarBuffers>>,
    num_nodes: usize,
}

impl AstarPool {
    pub fn new(num_nodes: usize, size: usize) -> Self {
        let buffers: Vec<AstarBuffers> = (0..size).map(|_| AstarBuffers::new(num_nodes)).collect();
        Self {
            buffers: std::sync::Mutex::new(buffers),
            num_nodes,
        }
    }

    /// Acquire a buffer set. If the pool is empty, allocates a new one.
    pub fn acquire(&self) -> AstarBuffers {
        let mut pool = self.buffers.lock().expect("pool lock poisoned");
        pool.pop()
            .unwrap_or_else(|| AstarBuffers::new(self.num_nodes))
    }

    /// Return a buffer set to the pool after resetting it.
    pub fn release(&self, mut buf: AstarBuffers) {
        buf.reset();
        let mut pool = self.buffers.lock().expect("pool lock poisoned");
        pool.push(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_new_initializes_correctly() {
        let buf = AstarBuffers::new(100);
        assert_eq!(buf.g_score.len(), 100);
        assert_eq!(buf.came_from.len(), 100);
        assert_eq!(buf.closed.len(), 100);
        assert!(buf.g_score.iter().all(|&v| v == f32::MAX));
        assert!(buf.came_from.iter().all(|&v| v == u32::MAX));
        assert!(buf.closed.iter().all(|&v| !v));
    }

    #[test]
    fn buffers_reset_clears_state() {
        let mut buf = AstarBuffers::new(10);
        buf.g_score[0] = 0.0;
        buf.came_from[0] = 5;
        buf.closed[0] = true;

        buf.reset();

        assert_eq!(buf.g_score[0], f32::MAX);
        assert_eq!(buf.came_from[0], u32::MAX);
        assert!(!buf.closed[0]);
    }

    #[test]
    fn pool_acquire_and_release() {
        let pool = AstarPool::new(50, 2);

        // Acquire a buffer
        let mut buf = pool.acquire();
        assert_eq!(buf.g_score.len(), 50);

        // Mutate it
        buf.g_score[0] = 1.0;
        buf.closed[0] = true;

        // Release resets the buffer
        pool.release(buf);

        // Re-acquire — should be reset
        let buf2 = pool.acquire();
        assert_eq!(buf2.g_score[0], f32::MAX);
        assert!(!buf2.closed[0]);
    }

    #[test]
    fn pool_concurrent_acquire() {
        let pool = std::sync::Arc::new(AstarPool::new(10, 2));
        let mut handles = vec![];

        for _ in 0..4 {
            let p = pool.clone();
            handles.push(std::thread::spawn(move || {
                let buf = p.acquire();
                assert_eq!(buf.g_score.len(), 10);
                std::thread::sleep(std::time::Duration::from_millis(10));
                p.release(buf);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }
}
