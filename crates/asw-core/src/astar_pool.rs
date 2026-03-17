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

/// Pool of reusable A* buffer sets backed by a tokio mpsc channel.
pub struct AstarPool {
    tx: tokio::sync::mpsc::Sender<AstarBuffers>,
    rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<AstarBuffers>>,
}

impl AstarPool {
    pub fn new(num_nodes: usize, size: usize) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(size);
        for _ in 0..size {
            tx.try_send(AstarBuffers::new(num_nodes))
                .expect("channel has capacity");
        }
        Self {
            tx,
            rx: tokio::sync::Mutex::new(rx),
        }
    }

    pub async fn acquire(&self) -> AstarBuffers {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .expect("pool channel closed")
    }

    pub async fn release(&self, mut buf: AstarBuffers) {
        buf.reset();
        let _ = self.tx.send(buf).await;
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

    #[tokio::test]
    async fn pool_acquire_and_release() {
        let pool = AstarPool::new(50, 2);

        // Acquire a buffer
        let mut buf = pool.acquire().await;
        assert_eq!(buf.g_score.len(), 50);

        // Mutate it
        buf.g_score[0] = 1.0;
        buf.closed[0] = true;

        // Release resets the buffer
        pool.release(buf).await;

        // Re-acquire — should be reset
        let buf2 = pool.acquire().await;
        assert_eq!(buf2.g_score[0], f32::MAX);
        assert!(!buf2.closed[0]);
    }
}
