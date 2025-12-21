// 公共导入：Any 用于类型擦除，Debug 用于调试输出，Hash/Hasher 用于哈希实现，
// ptr 用于指针比较，LazyLock 用于延迟初始化的全局驻留器。
// Common imports: Any for type erasure, Debug for formatting, Hash/Hasher for hashing,
// ptr for pointer operations, LazyLock for lazily initialized global interner.
use std::{
    any::Any,
    fmt::Debug,
    hash::{Hash, Hasher},
    ptr,
    sync::LazyLock,
};

// 从当前 crate 引入驻留相关的工具类型和 trait
// Import interning utilities from this crate
use crate::intern::{Internable, Interned, Interner};

pub use langgraph_macro::GraphLabel;
use variadics_please::all_tuples_with_size;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, GraphLabel)]
pub enum BaseGraphLabel {
    Start,
    End,
}

// 全局的 GraphLabel 驻留器：延迟初始化，用于为所有 GraphLabel trait 对象做驻留
// Global interner for GraphLabel trait objects, lazily initialized
static GRAPH_LABEL_INTERNER: LazyLock<Interner<dyn GraphLabel>> = LazyLock::new(Interner::new);

pub type InternedGraphLabel = Interned<dyn GraphLabel>;

// const N: 常量泛型，参数化是编译期常量值而不是类型参数
// const N: constant generic parameter, not a type parameter
pub trait IntoGraphNodeArray<const N: usize> {
    fn into_array(self) -> [InternedGraphLabel; N];
}

macro_rules! impl_render_label_tuples {
    ($N: expr, $(#[$meta:meta])* $(($T: ident, $I: ident)),*) => {
        $(#[$meta])*
        impl<$($T: GraphLabel),*> IntoGraphNodeArray<$N> for ($($T,)*) {
            #[inline]
            fn into_array(self) -> [InternedGraphLabel; $N] {
                let ($($I,)*) = self;
                [$($I.intern(), )*]
            }
        }
    }
}

all_tuples_with_size!(
    #[doc(fake_variadic)]
    impl_render_label_tuples,
    1,
    32,
    T,
    l
);

// DynEq：为 trait 对象提供基于运行时类型的相等比较接口
// DynEq: trait providing runtime type-based equality for trait objects
pub trait DynEq: Any {
    fn dyn_eq(&self, other: &dyn DynEq) -> bool;
}

impl<T> DynEq for T
where
    T: Any + Eq,
{
    fn dyn_eq(&self, other: &dyn DynEq) -> bool {
        // 将 DynEq trait 对象视为 Any，再尝试向具体类型 T 做向下转换，
        // 如果类型匹配则比较值是否相等，否则返回 false。
        // Treat the DynEq trait object as Any, downcast to concrete type T,
        // compare values when types match, otherwise return false.
        (other as &dyn Any).downcast_ref::<T>() == Some(self)
    }
}

// DynHash：在 DynEq 之上为 trait 对象提供动态哈希接口
// DynHash: provides dynamic hashing for trait objects on top of DynEq
pub trait DynHash: DynEq {
    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher);
}

impl<T> DynHash for T
where
    T: DynEq + Hash,
{
    fn dyn_hash(&self, mut state: &mut dyn Hasher) {
        // 首先使用具体类型 T 的 Hash 实现
        // First, use the concrete type T's Hash implementation
        T::hash(self, &mut state);
        // 再把运行时类型 ID 也写入哈希，确保不同类型不会产生相同哈希值
        // Then hash the runtime type id to distinguish different concrete types
        self.type_id().hash(&mut state);
    }
}

// GraphLabel：图中节点/边等使用的标签 trait，要求可调试、可发送同步、可哈希、可相等比较
// GraphLabel: label trait used in the graph, requiring Debug/Send/Sync/Hash/Eq semantics
pub trait GraphLabel: Debug + Send + Sync + DynHash + DynEq {
    // 返回一个 trait 对象的堆分配克隆
    // Return a heap-allocated clone of the trait object
    fn dyn_clone(&self) -> Box<dyn GraphLabel>;

    /// 返回当前标签的字符串表示
    /// Return a string representation of this label
    fn as_str(&self) -> &'static str;

    // 将自身驻留到全局 GraphLabel 驻留器中，返回 Interned<dyn GraphLabel>
    // Intern this value in the global GraphLabel interner and return Interned<dyn GraphLabel>
    fn intern(&self) -> Interned<dyn GraphLabel>
    where
        Self: Sized,
    {
        GRAPH_LABEL_INTERNER.intern(self)
    }
}

// 为 dyn GraphLabel 实现 PartialEq：委托给 DynEq 的动态相等比较
// Implement PartialEq for dyn GraphLabel via DynEq
impl PartialEq for dyn GraphLabel {
    fn eq(&self, other: &Self) -> bool {
        self.dyn_eq(other)
    }
}

// 为 dyn GraphLabel 实现 Eq：配合上面的 PartialEq 形成完全相等关系
// Implement Eq for dyn GraphLabel to complete equality semantics
impl Eq for dyn GraphLabel {}

// 为 dyn GraphLabel 实现 Hash：委托给 DynHash 的动态哈希逻辑
// Implement Hash for dyn GraphLabel via DynHash
impl Hash for dyn GraphLabel {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.dyn_hash(state);
    }
}

// 让 dyn GraphLabel 支持 Internable，以便用 Interner 进行驻留
// Implement Internable for dyn GraphLabel so it can be interned
impl Internable for dyn GraphLabel {
    fn leak(&self) -> &'static Self {
        // 通过 dyn_clone 克隆到堆上，再泄漏为 &'static 引用
        // Clone onto the heap via dyn_clone and leak as a &'static reference
        Box::leak(self.dyn_clone())
    }

    fn ref_eq(&self, other: &Self) -> bool {
        // 先比较运行时类型 ID，确保具体类型相同，
        // 再通过指针地址比较是否为同一实例
        // First compare runtime type ids, then compare addresses to check same instance
        self.type_id() == other.type_id()
            && ptr::addr_eq(ptr::from_ref::<Self>(self), ptr::from_ref::<Self>(other))
    }

    fn ref_hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // 将运行时类型 ID 写入哈希，然后将指针地址（擦除具体类型）写入哈希
        // Hash the runtime type id and then the erased pointer address
        self.type_id().hash(state);
        ptr::from_ref(self).cast::<()>().hash(state);
    }
}

/// 让 interned trait 对象本身也实现原本定义的标签 trait (GraphLabel)
/// Make Interned<dyn GraphLabel> itself implement the GraphLabel trait
impl GraphLabel for Interned<dyn GraphLabel> {
    fn dyn_clone(&self) -> Box<dyn GraphLabel> {
        // 将内部的 GraphLabel trait 对象克隆一份
        // Clone the inner GraphLabel trait object
        (**self).dyn_clone()
    }

    /// 返回当前标签的字符串表示
    /// Return a string representation of this label
    fn as_str(&self) -> &'static str {
        (**self).as_str()
    }

    fn intern(&self) -> Self {
        // 已经是驻留后的标签，直接返回自身即可
        // Already interned, just return itself
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    pub struct TestLabel;

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    pub enum TestEnumLabel {
        A,
        B,
    }

    // 非零大小的标签类型，用于测试指针身份相关逻辑
    // Non-zero-sized label type used to test pointer-identity-based behavior
    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    pub struct NonZstLabel(pub u32);

    #[test]
    fn test_graph_label_interner() {
        let label1 = TestLabel.intern();
        let label2 = TestLabel.intern();
        let label3 = TestLabel.intern();

        assert_eq!(label1, label2);
        assert_eq!(label1, label3);

        assert_eq!(label1.as_str(), "TestLabel");
        assert_eq!(label2.as_str(), "TestLabel");
        assert_eq!(label3.as_str(), "TestLabel");

        let enum_label1 = TestEnumLabel::A.intern();
        let enum_label2 = TestEnumLabel::A.intern();
        let enum_label3 = TestEnumLabel::B.intern();

        assert_eq!(enum_label1, enum_label2);
        assert_ne!(enum_label1, enum_label3);

        assert_eq!(enum_label1.as_str(), "A");
        assert_eq!(enum_label2.as_str(), "A");
        assert_eq!(enum_label3.as_str(), "B");
    }

    #[test]
    fn test_dyn_eq_across_types() {
        let a_val: i32 = 1;
        let b_val: i32 = 1;
        let c_val: i64 = 1;

        let a: &dyn DynEq = &a_val;
        let b: &dyn DynEq = &b_val;
        let c: &dyn DynEq = &c_val;

        // 同一具体类型且值相等时 dyn_eq 返回 true
        assert!(a.dyn_eq(b));
        // 不同具体类型即使“数值相等”也应返回 false
        assert!(!a.dyn_eq(c));
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ConstHashA(i32);

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ConstHashB(i32);

    impl Hash for ConstHashA {
        fn hash<H: Hasher>(&self, state: &mut H) {
            // 所有值都产生相同的哈希，以测试 DynHash 中 type_id 的影响
            0u8.hash(state);
        }
    }

    impl Hash for ConstHashB {
        fn hash<H: Hasher>(&self, state: &mut H) {
            // 所有值都产生相同的哈希，以测试 DynHash 中 type_id 的影响
            0u8.hash(state);
        }
    }

    #[test]
    fn test_dyn_hash_includes_type_id() {
        let a = ConstHashA(1);
        let b = ConstHashA(1);
        let c = ConstHashB(1);

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        let mut h3 = DefaultHasher::new();

        let da: &dyn DynHash = &a;
        let db: &dyn DynHash = &b;
        let dc: &dyn DynHash = &c;

        da.dyn_hash(&mut h1);
        db.dyn_hash(&mut h2);
        dc.dyn_hash(&mut h3);

        // 同一具体类型且逻辑相等的值应当产生相同哈希
        assert_eq!(h1.finish(), h2.finish());
        // 不同具体类型即使底层 Hash 实现完全相同，也应产生不同哈希
        assert_ne!(h1.finish(), h3.finish());
    }

    #[test]
    fn test_interned_graph_label_impl() {
        let interned = TestLabel.intern();

        // Interned<dyn GraphLabel> 也实现了 GraphLabel，可以调用 dyn_clone/as_str/intern
        let cloned: Box<dyn GraphLabel> = interned.dyn_clone();

        assert_eq!(interned.as_str(), "TestLabel");
        assert_eq!(cloned.as_str(), "TestLabel");

        // 通过 GraphLabel 的动态相等比较，两者在逻辑上应当相等
        let interned_dyn: &dyn GraphLabel = &*interned;
        let cloned_dyn: &dyn GraphLabel = cloned.as_ref();
        assert_eq!(interned_dyn, cloned_dyn);

        // 对已经驻留的标签再次调用 intern，应当是幂等的
        let reinterned = interned.intern();
        assert_eq!(interned, reinterned);
    }

    #[test]
    fn test_internable_dyn_graph_label_ref_eq_and_hash() {
        // 使用非零大小类型，避免 ZST Box 在实现上共享相同地址的特殊情况
        // Use a non-zero-sized type to avoid Box<ZST> sharing the same address
        let boxed_a: Box<dyn GraphLabel> = Box::new(NonZstLabel(1));
        let boxed_b: Box<dyn GraphLabel> = Box::new(NonZstLabel(1));

        let a: &dyn GraphLabel = boxed_a.as_ref();
        let b: &dyn GraphLabel = boxed_b.as_ref();

        // GraphLabel 的逻辑相等应当为 true
        assert_eq!(a, b);

        // Internable::ref_eq 使用指针身份判断，不同分配的实例应当为 false
        assert!(!a.ref_eq(b));

        // ref_hash 使用指针地址哈希，不同分配的实例通常会产生不同哈希值
        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();
        a.ref_hash(&mut h1);
        b.ref_hash(&mut h2);
        assert_ne!(h1.finish(), h2.finish());
    }
}
