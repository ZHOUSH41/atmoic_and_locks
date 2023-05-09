use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    mem::MaybeUninit,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    }, thread::{Thread, self}, marker::PhantomData,
};

pub struct Channel<T> {
    queue: Mutex<VecDeque<T>>,
    item_ready: Condvar,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            item_ready: Condvar::new(),
        }
    }

    pub fn send(&self, message: T) {
        self.queue.lock().unwrap().push_back(message);
        self.item_ready.notify_one();
    }

    pub fn receive(&self) -> T {
        let mut b = self.queue.lock().unwrap();
        loop {
            if let Some(message) = b.pop_front() {
                return message;
            } else {
                b = self.item_ready.wait(b).unwrap();
            }
        }
    }
}

pub struct OneShotChannelWithPanic<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    in_use: AtomicBool,
    ready: AtomicBool,
}

impl<T> OneShotChannelWithPanic<T> {
    pub fn new() -> Self {
        Self {
            message: UnsafeCell::new(MaybeUninit::uninit()),
            in_use: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        }
    }

    pub fn send(&self, message: T) {
        if self.in_use.swap(true, Ordering::Relaxed) {
            panic!("can't send more than one message!");
        }
        unsafe {
            (*self.message.get()).write(message);
        }
        self.ready.store(true, Ordering::Release)
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Panics if no message is available yet,
    /// or if the message was already consumed.
    /// Tip: Use `is_ready` to check first.
    pub fn receive(&self) -> T {
        if !self.ready.swap(false, Ordering::Acquire) {
            panic!("no message available!");
        }
        unsafe { (*self.message.get()).assume_init_read() }
    }
}

impl<T> Drop for OneShotChannelWithPanic<T> {
    fn drop(&mut self) {
        if *self.ready.get_mut() {
            unsafe { self.message.get_mut().assume_init_drop() }
        }
    }
}

unsafe impl<T> Sync for OneShotChannelWithPanic<T> where T: Send {}

/// 这里不用再public了
struct OneShotChannelWithArc<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

impl<T> OneShotChannelWithArc<T> {
    pub fn channel() -> (SenderWithArc<T>, ReceiverWithArc<T>) {
        let a = Arc::new(OneShotChannelWithArc {
            message: UnsafeCell::new(MaybeUninit::uninit()),
            ready: AtomicBool::new(false),
        });
        (
            SenderWithArc { channel: a.clone() },
            ReceiverWithArc { channel: a },
        )
    }
}
unsafe impl<T> Sync for OneShotChannelWithArc<T> where T: Send {}
pub struct SenderWithArc<T> {
    channel: Arc<OneShotChannelWithArc<T>>,
}

impl<T> SenderWithArc<T> {
    pub fn send(self, message: T) {
        unsafe { (*self.channel.message.get()).write(message) };
        self.channel.ready.store(true, Ordering::Release);
    }
}
pub struct ReceiverWithArc<T> {
    channel: Arc<OneShotChannelWithArc<T>>,
}

impl<T> ReceiverWithArc<T> {
    pub fn is_ready(&self) -> bool {
        self.channel.ready.load(Ordering::Relaxed)
    }
    pub fn receive(self) -> T {
        // 这里panic是防止未初始化，也就是防止receive在send之前调用
        if !self.channel.ready.swap(false, Ordering::Acquire) {
            panic!("no message available!");
        }
       unsafe { (*self.channel.message.get()).assume_init_read() } 
    }
}

impl<T> Drop for OneShotChannelWithArc<T> {
    fn drop(&mut self) {
        if *self.ready.get_mut() {
            unsafe { self.message.get_mut().assume_init_drop() }
        }
    }
}

unsafe impl<T> Sync for OneShotChannelWithBorrows<T> where T: Send {}

pub struct OneShotChannelWithBorrows<T> {
    message: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

impl<T> OneShotChannelWithBorrows<T> {
    pub fn new() -> Self {
        Self { message: UnsafeCell::new(MaybeUninit::uninit()), ready: AtomicBool::new(false) }
    }

    pub fn split<'a>(&'a mut self) -> (SenderWithBorrows<'a, T>, ReceiverWithBorrows<'a, T>) {
        *self = Self::new();
        (SenderWithBorrows{channel: self, receving_thread: thread::current()}, ReceiverWithBorrows{channel: self, _no_send: PhantomData})
    }
}

pub struct SenderWithBorrows<'a, T> {
    channel: &'a OneShotChannelWithBorrows<T>,
    // 为了unpark对应的线程
    receving_thread:Thread,
}

impl<T> SenderWithBorrows<'_, T> {
    pub fn send(self, message: T) {
        unsafe { (*self.channel.message.get()).write(message) };
        self.channel.ready.store(true, Ordering::Release);
        self.receving_thread.unpark();
    }
}

pub struct ReceiverWithBorrows<'a, T> {
    channel: &'a OneShotChannelWithBorrows<T>,
    // marker type 表明为不能send的类型
    _no_send: PhantomData<* const ()>
}

impl<T> ReceiverWithBorrows<'_, T> {
    pub fn is_ready(&self) -> bool {
        self.channel.ready.load(Ordering::Relaxed)
    }

    pub fn receive(self) -> T {
        if !self.channel.ready.swap(false, Ordering::Acquire) {
            thread::park();
        }
        unsafe { (*self.channel.message.get()).assume_init_read() }
    }
}
impl<T> Drop for OneShotChannelWithBorrows<T> {
    fn drop(&mut self) {
        if *self.ready.get_mut() {
            unsafe { self.message.get_mut().assume_init_drop() }
} 
    }
}



fn main() {}

#[cfg(test)]
mod test {
    use std::thread;

    use super::*;

    #[test]
    fn mutex_channel_works() {}

    #[test]
    fn one_shot_channel_with_panic_works() {
        let channel = OneShotChannelWithPanic::new();
        let t = thread::current();
        thread::scope(|s| {
            s.spawn(|| {
                channel.send("hello world!");
                t.unpark();
            });
            while !channel.is_ready() {
                thread::park();
            }
            println!("Hello, world!");
            assert_eq!(channel.receive(), "hello world!");
        });
    }

    #[test]
    fn one_shot_channel_with_arc_works() {
        thread::scope(|s| {
            let (sender, receiver) = OneShotChannelWithArc::channel();
            let t = thread::current();
            s.spawn(move || {
                sender.send("hello world!");
                t.unpark();
            });
            while !receiver.is_ready() {
                thread::park();
            }
            assert_eq!(receiver.receive(), "hello world!");
        });
    }

    #[test]
    fn one_shot_channel_with_borrow_works() {
        let mut channel = OneShotChannelWithBorrows::new();
        thread::scope(|s| {
            let (sender, receiver) = channel.split();
            let t = thread::current();
            s.spawn(move || {
                sender.send("hello world!");
                t.unpark();
            });
            while !receiver.is_ready() {
                thread::park();
            }
            assert_eq!(receiver.receive(), "hello world!");
        });
    }

    #[test]
    fn one_shot_channel_with_borrow_block_works() {
        let mut channel = OneShotChannelWithBorrows::new();
        thread::scope(|s| {
            let (sender, receiver) = channel.split();
            s.spawn(move || {
                sender.send("hello world!");
            });
            assert_eq!(receiver.receive(), "hello world!");
        });
    }
}
