#[cfg(any(feature = "alloc", feature = "std"))]
mod alloc;
#[cfg(not(any(feature = "alloc", feature = "std")))]
mod core;
