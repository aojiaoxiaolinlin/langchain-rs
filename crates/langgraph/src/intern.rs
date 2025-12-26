use std::{
    collections::HashSet,
    fmt::Debug,
    hash::Hash,
    ops::Deref,
    sync::{PoisonError, RwLock},
};

// Interned<T>：包装一个 &'static T 的类型，用于保存被驻留（intern）后的静态引用
// Interned<T>: wraps a &'static T used to store a statically interned reference
pub struct Interned<T: ?Sized + 'static>(pub &'static T);

// 为 Interned<T> 实现 Deref，使其可以像 &T 一样被解引用使用
// Implement Deref for Interned<T> so it can be used like &T
impl<T: ?Sized> Deref for Interned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

// 为 Interned<T> 实现 Clone：克隆只是复制内部的静态引用本身
// Implement Clone for Interned<T>: cloning just copies the inner static reference
impl<T: ?Sized + 'static> Clone for Interned<T> {
    fn clone(&self) -> Self {
        *self
    }
}

// 为 Interned<T> 实现 Copy：因为内部仅保存一个 &'static T，可以按值复制
// Implement Copy for Interned<T>: it only holds a &'static T so it is copyable
impl<T: ?Sized + 'static> Copy for Interned<T> {}

// Two Interned<T> should only be equal if they are clones from the same instance.
// Therefore, we only use the pointer to determine equality.
// 两个 Interned<T> 只有在它们来自同一个实例的克隆时才应当相等，
// 因此这里仅通过指针来判断相等性。
// Implement PartialEq for Interned<T> using pointer-based equality
impl<T: ?Sized + Internable> PartialEq for Interned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.ref_eq(other.0)
    }
}

// 为 Interned<T> 实现 Eq，配合 PartialEq 形成完全相等关系
// Implement Eq for Interned<T> to complete equality semantics
impl<T: ?Sized + Internable> Eq for Interned<T> {}

// Important: This must be kept in sync with the PartialEq/Eq implementation
// 重要：此实现必须与 PartialEq/Eq 的实现保持一致
// Implement Hash for Interned<T> consistent with its equality (pointer-based)
impl<T: ?Sized + Internable> Hash for Interned<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.ref_hash(state);
    }
}

// 为 Interned<T> 实现 Debug：直接委托给内部值的 Debug 实现
// Implement Debug for Interned<T> by delegating to the inner value
impl<T: ?Sized + Debug> Debug for Interned<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

// Implement Ord for Interned<T> based on pointer address
// This is required for sorting/deduping collections of Interned<T>
impl<T: ?Sized + Internable> PartialOrd for Interned<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ?Sized + Internable> Ord for Interned<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.0 as *const T)
            .cast::<()>()
            .cmp(&(other.0 as *const T).cast::<()>())
    }
}

// 从 &Interned<T> 创建 Interned<T>：按值复制内部静态引用
// Implement From<&Interned<T>> for Interned<T> to copy the inner reference
impl<T> From<&Interned<T>> for Interned<T> {
    fn from(value: &Interned<T>) -> Self {
        *value
    }
}

// Internable：描述可以被驻留（intern）的类型
// 包含将自身泄漏为 &'static Self 及基于引用的相等和哈希方法
// Internable: describes types that can be interned with leak/eq/hash by reference
pub trait Internable: Hash + Eq {
    /// Creates a static reference to `self`, possibly leaking memory.
    /// 创建指向 `self` 的静态引用，可能会导致内存泄漏。
    fn leak(&self) -> &'static Self;

    /// Returns `true` if the two references point to the same value.
    /// 如果两个引用指向同一个值，则返回 `true`。
    fn ref_eq(&self, other: &Self) -> bool;

    /// Feeds the reference to the hasher.
    /// 将该引用的指针信息写入哈希器。
    fn ref_hash<H: core::hash::Hasher>(&self, state: &mut H);
}

// 为 str 实现 Internable，使字符串切片可以被驻留并通过指针比较和哈希
// Implement Internable for str so string slices can be interned and compared/hashed by pointer
impl Internable for str {
    fn leak(&self) -> &'static Self {
        let str = self.to_owned().into_boxed_str();
        Box::leak(str)
    }

    fn ref_eq(&self, other: &Self) -> bool {
        self.as_ptr() == other.as_ptr() && self.len() == other.len()
    }

    fn ref_hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_ptr().hash(state);
        self.len().hash(state);
    }
}

// Interner<T>：保存 &'static T 的集合，通过读写锁实现线程安全的驻留表
// Interner<T>: holds a thread-safe set of &'static T used for interning
pub struct Interner<T: ?Sized + 'static>(RwLock<HashSet<&'static T>>);

impl<T: ?Sized> Interner<T> {
    /// Creates a new empty interner
    /// 创建一个新的空的驻留器。
    pub fn new() -> Self {
        Self(RwLock::new(HashSet::new()))
    }
}

impl<T: Internable + ?Sized> Interner<T> {
    /// Return the [`Interned<T>`] corresponding to `value`.
    /// 返回与 `value` 对应的 [`Interned<T>`]。
    ///
    /// If it is called the first time for `value`, it will possibly leak the value and return an
    /// [`Interned<T>`] using the obtained static reference. Subsequent calls for the same `value`
    /// will return [`Interned<T>`] using the same static reference.
    /// 如果这是首次对某个 `value` 调用，本函数可能会泄漏该值并返回使用得到的
    /// 静态引用构造的 [`Interned<T>`]。之后对同一个 `value` 的调用将复用同一个
    /// 静态引用并返回相同的 [`Interned<T>`]。
    pub fn intern(&self, value: &T) -> Interned<T> {
        {
            // 首先尝试在读锁下查找已存在的静态引用，避免不必要的写锁
            // First, try to find an existing static reference under a read lock
            let set = self.0.read().unwrap_or_else(PoisonError::into_inner);

            if let Some(value) = set.get(value) {
                return Interned(*value);
            }
        }

        {
            // 如果读阶段未命中，则在写锁下再次检查并在需要时泄漏并插入新值
            // If not found, acquire a write lock to insert a leaked value if necessary
            let mut set = self.0.write().unwrap_or_else(PoisonError::into_inner);

            if let Some(value) = set.get(value) {
                Interned(*value)
            } else {
                let leaked = value.leak();
                set.insert(leaked);
                Interned(leaked)
            }
        }
    }
}

// 为 Interner<T> 提供默认实现，默认创建一个新的空驻留器
// Provide Default for Interner<T> that creates a new empty interner
impl<T: ?Sized> Default for Interner<T> {
    fn default() -> Self {
        Self::new()
    }
}
