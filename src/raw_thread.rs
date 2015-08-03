use std::boxed::FnBox;
use std::mem;
use std::thread;

pub unsafe fn spawn<'a, F>(f: F) -> thread::JoinHandle<()> where F: FnOnce() + 'a {
    let closure: Box<FnBox() + 'a> = Box::new(f);
    let closure: Box<FnBox() + Send> = mem::transmute(closure);
    thread::spawn(closure)
}
