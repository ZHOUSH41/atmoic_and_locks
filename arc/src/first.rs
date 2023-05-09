use std::{
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{fence, AtomicUsize, Ordering},
    usize,
};

struct ArcData<T> {
    ref_count: AtomicUsize,
    data: T,
}

pub struct Arc<T> {
    ptr: NonNull<ArcData<T>>,
}

/// Sending an Arc<T> across threads results in a T object being shared, requiring T to be Sync.
/// Similarly, sending an Arc<T> across threads could result in another thread dropping that T,
/// effectively transferring it to the other thread, requiring T to be Send. In other words,
///  Arc<T> should be Send if and only if T is both Send and Sync. The exact same holds for Sync,
/// since a shared &Arc<T> can be cloned into a new Arc<T>.
///
/// Arc Send其实Send是一个共享指针，Send就是共享了T，T需要保证Sync；Arc Send也会导致另一个线程释放T，需要T是Send
/// Arc Sync就是&Arc<T>也就是Clone to Arc<T>，同样的保证T Send+Sync
unsafe impl<T> Send for Arc<T> where T: Send + Sync {}
unsafe impl<T> Sync for Arc<T> where T: Send + Sync {}

impl<T> Arc<T> {
    pub fn new(data: T) -> Self {
        /// Box leak保证放弃了排他的所有权，Box new是新分配一个内存
        Self {
            ptr: NonNull::from(Box::leak(Box::new(ArcData {
                ref_count: AtomicUsize::new(1),
                data,
            }))),
        }
    }

    fn data(&self) -> &ArcData<T> {
        /// 这里可以使用unsafe的原因是，Arc存在就保证了ptr非空，这时候就可以正常访问
        unsafe {
            self.ptr.as_ref()
        }
    }

    /// 这里使用静态方法，是为了避免混淆T实现的 a.get_mut()，专门写成Arc::get_mut
    pub fn get_mut(arc: &mut Self) -> Option<&mut T> {
        if arc.data().ref_count.load(Ordering::Relaxed) == 1 {
            fence(Ordering::Acquire);
            // Safety: Nothing else can access the data, since
            // there's only one Arc, to which we have exclusive access.
            unsafe { Some(&mut arc.ptr.as_mut().data) }
        } else {
            None
        }
    }
}

impl<T> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data().data
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        unsafe {
            // handle overflows
            if self.data().ref_count.fetch_add(1, Ordering::Relaxed) > usize::MAX / 2 {
                std::process::abort();
            }
        }
        Self { ptr: self.ptr }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        if self.data().ref_count.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            unsafe {
                drop(Box::from_raw(self.ptr.as_ptr()));
            }
        }
    }
}

// TODO: 有条件的实现mut ref


/// 多线程的测试是一个很难的事情，这里建议使用Miri来测试对应的正确性
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        static NUM_DROPS: AtomicUsize = AtomicUsize::new(0);
        struct DetectDrop;
        impl Drop for DetectDrop {
            fn drop(&mut self) {
                NUM_DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Create two Arcs sharing an object containing a string // and a DetectDrop, to detect when it's dropped.
        let x = Arc::new(("hello", DetectDrop));
        let y = x.clone();
        // Send x to another thread, and use it there.
        let t = std::thread::spawn(move || {
            assert_eq!(x.0, "hello");
        });
        // In parallel, y should still be usable here.
        assert_eq!(y.0, "hello");
        // Wait for the thread to finish.
        t.join().unwrap();
        // One Arc, x, should be dropped by now.
        // We still have y, so the object shouldn't have been dropped yet. assert_eq!(NUM_DROPS.load(Relaxed), 0);
        // Drop the remaining `Arc`.
        drop(y);
        // Now that `y` is dropped too,
        // the object should've been dropped. assert_eq!(NUM_DROPS.load(Relaxed), 1);
    }
}
