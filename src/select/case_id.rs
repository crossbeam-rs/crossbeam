#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    pub id: usize,
}

impl CaseId {
    #[inline]
    pub fn new(id: usize) -> Self {
        CaseId { id }
    }

    #[inline]
    pub fn none() -> Self {
        CaseId { id: 0 }
    }

    #[inline]
    pub fn abort() -> Self {
        CaseId { id: 1 }
    }
}
