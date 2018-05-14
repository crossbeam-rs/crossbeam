#[allow(dead_code)]
pub struct TestType {
    index: usize,
    #[cfg(feature = "medium_size")]
    medium: [usize; 31],
    #[cfg(feature = "large_size")]
    large: [usize; 63],
}

impl TestType {
    pub fn new(index: usize) -> TestType {
        TestType {
            index,
            #[cfg(feature = "medium_size")]
            medium: [0; 31],
            #[cfg(feature = "large_size")]
            large: [0; 63],
        }
    }
}