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
    pub fn closed() -> Self {
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
    pub fn send(channel_address: usize) -> Self {
        CaseId { id: channel_address }
    }

    #[inline]
    pub fn recv(channel_address: usize) -> Self {
        CaseId { id: channel_address | 1 }
    }

    #[inline]
    pub fn new(id: usize) -> Self {
        CaseId { id }
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
