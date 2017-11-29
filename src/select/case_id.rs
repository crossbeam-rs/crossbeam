use channel::Channel;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    id: usize,
}

impl CaseId {
    #[inline]
    pub fn none() -> Self {
        CaseId { id: 0 }
    }

    #[inline]
    pub fn abort() -> Self {
        CaseId { id: 1 }
    }

    #[inline]
    pub fn disconnected() -> Self {
        CaseId { id: 2 }
    }

    #[inline]
    pub fn would_block() -> Self {
        CaseId { id: 3 }
    }

    #[inline]
    pub fn timed_out() -> Self {
        CaseId { id: 4 }
    }

    #[inline]
    pub fn send<T>(chan: &Channel<T>) -> Self {
        let addr = chan as *const Channel<T> as usize;
        CaseId { id: addr }
    }

    #[inline]
    pub fn recv<T>(chan: &Channel<T>) -> Self {
        let addr = chan as *const Channel<T> as usize;
        CaseId { id: addr | 1 }
    }

    #[inline]
    pub fn is_send(&self) -> bool {
        self.id >= 10 && self.id & 1 == 0
    }

    #[inline]
    pub fn is_recv(&self) -> bool {
        self.id >= 10 && self.id & 1 == 1
    }
}

impl From<usize> for CaseId {
    #[inline]
    fn from(id: usize) -> Self {
        CaseId { id }
    }
}

impl Into<usize> for CaseId {
    #[inline]
    fn into(self) -> usize {
        self.id
    }
}
