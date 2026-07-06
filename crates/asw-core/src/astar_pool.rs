/// Pre-allocated buffers for A* search to avoid per-request allocation.
///
/// Per-node state (`g_score`, `came_from`, `closed`, `h_score`) is guarded by
/// a generation counter (`gen`/`current_gen`) instead of being eagerly
/// cleared: a node's entry is only meaningful when `gen[node] ==
/// current_gen`. `reset()` therefore just bumps `current_gen` (O(1)) rather
/// than rewriting the whole graph's worth of state on every request; callers
/// (the A* search in `routing.rs`) must go through [`AstarBuffers::touch`]
/// before treating an entry as valid, and must reinitialize it (as `touch`
/// does for g_score/came_from/closed) the first time a node is seen in a
/// generation.
pub struct AstarBuffers {
    pub(crate) g_score: Vec<f32>,
    pub(crate) came_from: Vec<u32>,
    pub(crate) closed: Vec<bool>,
    /// Cached A* heuristic (haversine distance to goal) per node, valid only
    /// under the same generation guard as the other fields. Populated lazily
    /// by the caller on first `touch()` of a node.
    pub(crate) h_score: Vec<f32>,
    gen: Vec<u32>,
    current_gen: u32,
}

impl AstarBuffers {
    pub fn new(num_nodes: usize) -> Self {
        Self {
            g_score: vec![f32::MAX; num_nodes],
            came_from: vec![u32::MAX; num_nodes],
            closed: vec![false; num_nodes],
            h_score: vec![0.0; num_nodes],
            gen: vec![0; num_nodes],
            // Start at 1 so the all-zero `gen` vec from a fresh allocation is
            // immediately treated as "never touched" (0 != 1).
            current_gen: 1,
        }
    }

    /// O(1) reset: invalidate all per-node state by bumping the generation
    /// counter. Falls back to a full O(num_nodes) clear of `gen` only on the
    /// rare u32 wraparound, then restarts generations at 1.
    pub fn reset(&mut self) {
        if self.current_gen == u32::MAX {
            self.gen.fill(0);
            self.current_gen = 1;
        } else {
            self.current_gen += 1;
        }
    }

    /// Ensure `node`'s slot is valid for the current generation. Returns
    /// `true` if this is the first touch this generation (in which case the
    /// caller is responsible for populating any derived per-node state, e.g.
    /// the cached heuristic in `h_score`) — `g_score`/`came_from`/`closed`
    /// are reset to their defaults here unconditionally on first touch.
    ///
    /// Must be called before reading or writing `g_score`, `came_from`,
    /// `closed`, or `h_score` for a given node in a given generation.
    #[inline]
    pub(crate) fn touch(&mut self, node: u32) -> bool {
        let idx = node as usize;
        if self.gen[idx] != self.current_gen {
            self.gen[idx] = self.current_gen;
            self.g_score[idx] = f32::MAX;
            self.came_from[idx] = u32::MAX;
            self.closed[idx] = false;
            true
        } else {
            false
        }
    }

    /// Test-only hook to drive `current_gen` near `u32::MAX` so the
    /// wraparound path in `reset()` can be exercised without looping
    /// billions of times.
    #[cfg(test)]
    pub(crate) fn set_current_gen_for_test(&mut self, gen: u32) {
        self.current_gen = gen;
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
        let mut buf = AstarBuffers::new(100);
        assert_eq!(buf.g_score.len(), 100);
        assert_eq!(buf.came_from.len(), 100);
        assert_eq!(buf.closed.len(), 100);

        // Every node reads as "unset" the first time it's touched.
        for &i in &[0u32, 50, 99] {
            assert!(
                buf.touch(i),
                "node {i} should be a first touch on a fresh buffer"
            );
            assert_eq!(buf.g_score[i as usize], f32::MAX);
            assert_eq!(buf.came_from[i as usize], u32::MAX);
            assert!(!buf.closed[i as usize]);
        }
    }

    #[test]
    fn buffers_reset_clears_state() {
        let mut buf = AstarBuffers::new(10);
        assert!(buf.touch(0));
        buf.g_score[0] = 0.0;
        buf.came_from[0] = 5;
        buf.closed[0] = true;

        buf.reset();

        assert!(
            buf.touch(0),
            "node touched before a previous generation should present as untouched after reset"
        );
        assert_eq!(buf.g_score[0], f32::MAX);
        assert_eq!(buf.came_from[0], u32::MAX);
        assert!(!buf.closed[0]);
    }

    #[test]
    fn reset_is_o1_generation_bump_not_full_clear() {
        let mut buf = AstarBuffers::new(5);
        assert!(buf.touch(2));
        buf.g_score[2] = 7.0;
        buf.closed[2] = true;

        buf.reset();

        // reset() must not eagerly rewrite the raw arrays — the old values
        // are still physically present until the node is touched again.
        assert_eq!(buf.g_score[2], 7.0);
        assert!(buf.closed[2]);

        // But logically, the generation guard treats it as fresh: touch()
        // reports a first touch and reinitializes the entry lazily.
        assert!(buf.touch(2));
        assert_eq!(buf.g_score[2], f32::MAX);
        assert!(!buf.closed[2]);
    }

    #[test]
    fn reset_handles_generation_wraparound() {
        let mut buf = AstarBuffers::new(10);
        buf.set_current_gen_for_test(u32::MAX);

        assert!(buf.touch(3));
        buf.g_score[3] = 42.0;
        buf.closed[3] = true;
        // A node NOT touched under this generation.
        assert_eq!(buf.g_score[4], f32::MAX);

        buf.reset(); // wraps: full clear of `gen`, current_gen restarts at 1

        // Previously-touched node must present as fresh again, not as
        // "already valid" due to reused generation numbers.
        assert!(buf.touch(3));
        assert_eq!(buf.g_score[3], f32::MAX);
        assert!(!buf.closed[3]);

        // A second reset after wraparound should go back to simple bumps.
        buf.reset();
        assert!(buf.touch(3));
    }

    #[test]
    fn pool_acquire_and_release() {
        let pool = AstarPool::new(50, 2);

        // Acquire a buffer
        let mut buf = pool.acquire();
        assert_eq!(buf.g_score.len(), 50);

        // Mutate it
        assert!(buf.touch(0));
        buf.g_score[0] = 1.0;
        buf.closed[0] = true;

        // Release resets the buffer
        pool.release(buf);

        // Re-acquire — should present as reset
        let mut buf2 = pool.acquire();
        assert!(buf2.touch(0));
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
